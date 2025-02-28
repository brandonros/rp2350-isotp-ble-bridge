use crate::can_manager::CanMessage;
use crate::channels::{ISOTP_BLE_CHANNEL, ISOTP_CAN_CHANNEL};
use crate::isotp_handler::IsotpHandler;
use crate::{ble_protocol::*, can_manager};
use defmt::{error, info, Format};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;

// Create a static shared manager
static ISOTP_MANAGER: Mutex<ThreadModeRawMutex, IsoTpManager> = Mutex::new(IsoTpManager::new());

/// Error type for message parsing
#[derive(Debug, Format)]
pub enum ManagerError {
    FailedToInsertFilter,
    FilterAlreadyExists,
    InvalidOffset,
    FilterNotFound,
    FailedToSendMessage,
}

const MAX_HANDLERS: usize = 4;

pub struct IsoTpManager {
    handlers: heapless::FnvIndexMap<u32, IsotpHandler, MAX_HANDLERS>,
    tx_buffer: heapless::Vec<u8, 4096>,
}

impl IsoTpManager {
    pub const fn new() -> Self {
        Self {
            handlers: heapless::FnvIndexMap::<u32, IsotpHandler, MAX_HANDLERS>::new(),
            tx_buffer: heapless::Vec::new(),
        }
    }

    pub async fn handle_ble_message(
        &mut self,
        parsed: &ParsedBleMessage,
    ) -> Result<(), ManagerError> {
        match parsed {
            ParsedBleMessage::UploadIsotpChunk(upload_chunk_command) => {
                let offset = upload_chunk_command.offset;
                let chunk_length = upload_chunk_command.chunk_length;
                let chunk = upload_chunk_command.chunk.as_slice();

                // check if offset is valid
                if offset + chunk_length > self.tx_buffer.len() as u16 {
                    return Err(ManagerError::InvalidOffset);
                }

                // Copy chunk into tx buffer at offset
                let start = offset as usize;
                let end = start + chunk_length as usize;
                self.tx_buffer[start..end].copy_from_slice(chunk);

                Ok(())
            }
            ParsedBleMessage::SendIsotpBuffer(send_isotp_buffer_command) => {
                let payload_length = send_isotp_buffer_command.total_length;
                let request_arbitration_id = u32::from_be_bytes([
                    self.tx_buffer[0],
                    self.tx_buffer[1],
                    self.tx_buffer[2],
                    self.tx_buffer[3],
                ]);
                let reply_arbitration_id = u32::from_be_bytes([
                    self.tx_buffer[4],
                    self.tx_buffer[5],
                    self.tx_buffer[6],
                    self.tx_buffer[7],
                ]);
                let _msg_length = payload_length - 8;
                let msg = &self.tx_buffer[8..];

                info!(
                    "Sending message to {}:{}",
                    request_arbitration_id, reply_arbitration_id
                );

                // lookup filter_index from request_arbitration_id
                let filter_index = self.handlers.iter().position(|(_key, handler)| {
                    handler.request_arbitration_id == request_arbitration_id
                        && handler.reply_arbitration_id == reply_arbitration_id
                });
                if filter_index.is_none() {
                    return Err(ManagerError::FilterNotFound);
                }
                let filter_index = filter_index.unwrap() as u32;

                // get handler by index
                let handler = self.handlers.get_mut(&filter_index).unwrap();

                // send message
                match handler.send_message(reply_arbitration_id, msg).await {
                    true => Ok(()),
                    false => Err(ManagerError::FailedToSendMessage),
                }
            }
            ParsedBleMessage::StartPeriodicMessage(_start_periodic_message_command) => {
                todo!()
            }
            ParsedBleMessage::StopPeriodicMessage(_stop_periodic_message_command) => {
                todo!()
            }
            ParsedBleMessage::ConfigureIsotpFilter(configure_filter_command) => {
                info!(
                    "Configuring filter: {} {} {}",
                    configure_filter_command.filter_id,
                    configure_filter_command.request_arbitration_id,
                    configure_filter_command.reply_arbitration_id
                );

                // check if already exists
                if self
                    .handlers
                    .contains_key(&configure_filter_command.filter_id)
                {
                    return Err(ManagerError::FilterAlreadyExists);
                }

                // register filter with can_manager
                if !can_manager::register_isotp_filter(
                    configure_filter_command.reply_arbitration_id,
                ) {
                    return Err(ManagerError::FailedToInsertFilter);
                }

                // insert handler
                match self.handlers.insert(
                    configure_filter_command.filter_id,
                    IsotpHandler::new(
                        configure_filter_command.request_arbitration_id,
                        configure_filter_command.reply_arbitration_id,
                    ),
                ) {
                    Ok(_) => Ok(()),
                    Err(_) => Err(ManagerError::FailedToInsertFilter),
                }
            }
        }
    }

    async fn handle_can_frame(&mut self, id: u32, data: &[u8]) {
        for (_filter_id, handler) in self.handlers.iter_mut() {
            if handler.request_arbitration_id == id || handler.reply_arbitration_id == id {
                handler.handle_received_frame(id, data).await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn isotp_manager_can_task() {
    info!("IsoTP manager CAN task started");

    loop {
        let can_message = ISOTP_CAN_CHANNEL.receive().await;
        // Brief critical section
        ISOTP_MANAGER
            .lock()
            .await
            .handle_can_frame(can_message.id, &can_message.data)
            .await;
    }
}

#[embassy_executor::task]
pub async fn isotp_manager_ble_task() {
    info!("IsoTP manager BLE task started");

    loop {
        let parsed_message = ISOTP_BLE_CHANNEL.receive().await;
        // Brief critical section
        match ISOTP_MANAGER
            .lock()
            .await
            .handle_ble_message(&parsed_message)
            .await
        {
            Ok(_) => (),
            Err(e) => error!("Error handling BLE message: {:?}", e),
        }
    }
}

// Helper functions to send messages to the IsoTP task
pub async fn handle_ble_message(message: ParsedBleMessage) {
    ISOTP_BLE_CHANNEL.send(message).await;
}

pub async fn handle_can_message(message: CanMessage) {
    ISOTP_CAN_CHANNEL.send(message).await;
}
