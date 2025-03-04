use crate::can_manager::CanMessage;
use crate::channels::{ISOTP_BLE_CHANNEL, ISOTP_CAN_CHANNEL};
use crate::isotp_handler::IsotpHandler;
use crate::{ble_protocol::*, can_manager, led};
use defmt::{debug, error, info, Format};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;

// Create a static shared manager
static ISOTP_BLE_BRIDGE: Mutex<ThreadModeRawMutex, IsotpBleBridge> =
    Mutex::new(IsotpBleBridge::new());

/// Error type for message parsing
#[derive(Debug, Format)]
pub enum ManagerError {
    FailedToInsertFilter,
    FilterAlreadyExists,
    InvalidOffset,
    InvalidPayloadLength,
    FilterNotFound,
    FailedToSendMessage,
}

const MAX_HANDLERS: usize = 4;
const MAX_TX_BUFFER_SIZE: usize = 4096;

pub struct IsotpBleBridge {
    isotp_handlers: heapless::FnvIndexMap<u32, IsotpHandler, MAX_HANDLERS>,
    isotp_tx_buffer: heapless::Vec<u8, MAX_TX_BUFFER_SIZE>,
}

impl IsotpBleBridge {
    pub const fn new() -> Self {
        Self {
            isotp_handlers: heapless::FnvIndexMap::<u32, IsotpHandler, MAX_HANDLERS>::new(),
            isotp_tx_buffer: heapless::Vec::new(),
        }
    }

    pub async fn handle_ble_message(
        &mut self,
        parsed: &ParsedBleMessage,
    ) -> Result<(), ManagerError> {
        match parsed {
            ParsedBleMessage::UploadIsotpChunk(upload_chunk_command) => {
                debug!("UploadIsotpChunk: {:?}", upload_chunk_command);

                let offset = upload_chunk_command.offset;
                let chunk_length = upload_chunk_command.chunk_length;
                let chunk = upload_chunk_command.chunk.as_slice();

                // check if offset + length would exceed max buffer size
                if offset + chunk_length > MAX_TX_BUFFER_SIZE as u16 {
                    return Err(ManagerError::InvalidOffset);
                }

                // Ensure buffer is large enough
                let required_len = (offset as usize) + (chunk_length as usize);
                match self.isotp_tx_buffer.resize(required_len, 0) {
                    Ok(_) => (),
                    Err(_) => return Err(ManagerError::InvalidOffset),
                }

                // Copy chunk into buffer
                let start = offset as usize;
                let end = start + chunk_length as usize;
                self.isotp_tx_buffer[start..end].copy_from_slice(chunk);

                Ok(())
            }
            ParsedBleMessage::SendIsotpBuffer(send_isotp_buffer_command) => {
                debug!("SendIsotpBuffer: {:?}", send_isotp_buffer_command);

                let payload_length = send_isotp_buffer_command.total_length;
                let request_arbitration_id = u32::from_be_bytes([
                    self.isotp_tx_buffer[0],
                    self.isotp_tx_buffer[1],
                    self.isotp_tx_buffer[2],
                    self.isotp_tx_buffer[3],
                ]);
                let reply_arbitration_id = u32::from_be_bytes([
                    self.isotp_tx_buffer[4],
                    self.isotp_tx_buffer[5],
                    self.isotp_tx_buffer[6],
                    self.isotp_tx_buffer[7],
                ]);
                let msg = &self.isotp_tx_buffer[8..];

                // subtract 8 bytes for the arbitration ids
                if msg.len() != (payload_length - 8) as usize {
                    debug!(
                        "Invalid payload length: {:?}, {:?}, {:02x}",
                        payload_length,
                        msg.len(),
                        msg
                    );
                    return Err(ManagerError::InvalidPayloadLength);
                }

                info!(
                    "Sending message to {:x}:{:x} {:02x}",
                    request_arbitration_id, reply_arbitration_id, msg
                );

                // Find the handler that matches both IDs
                let matching_handler = self.isotp_handlers.iter_mut().find(|(_key, handler)| {
                    handler.request_arbitration_id == request_arbitration_id
                        && handler.reply_arbitration_id == reply_arbitration_id
                });

                let handler = match matching_handler {
                    Some((_key, handler)) => handler,
                    None => return Err(ManagerError::FilterNotFound),
                };

                // send message
                match handler
                    .send_isotp_message(request_arbitration_id, msg)
                    .await
                {
                    true => (),
                    false => return Err(ManagerError::FailedToSendMessage),
                }

                // flush tx buffer
                self.isotp_tx_buffer.clear();

                Ok(())
            }
            ParsedBleMessage::StartPeriodicIsotpMessage(_start_periodic_message_command) => {
                todo!()
            }
            ParsedBleMessage::StopPeriodicIsotpMessage(_stop_periodic_message_command) => {
                todo!()
            }
            ParsedBleMessage::ConfigureIsotpFilter(configure_filter_command) => {
                debug!("ConfigureIsotpFilter: {:?}", configure_filter_command);

                // check if already exists
                if self
                    .isotp_handlers
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
                match self.isotp_handlers.insert(
                    configure_filter_command.filter_id,
                    IsotpHandler::new(
                        configure_filter_command.request_arbitration_id,
                        configure_filter_command.reply_arbitration_id,
                    ),
                ) {
                    Ok(_) => (),
                    Err(_) => return Err(ManagerError::FailedToInsertFilter),
                }

                Ok(())
            }
        }
    }

    async fn handle_can_frame(&mut self, id: u32, data: &[u8]) {
        for (_filter_id, handler) in self.isotp_handlers.iter_mut() {
            if handler.request_arbitration_id == id || handler.reply_arbitration_id == id {
                handler.handle_received_can_frame(id, data).await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn isotp_ble_bridge_can_rx_task() {
    info!("BLE IsoTP bridge CAN task started");

    loop {
        let can_message = ISOTP_CAN_CHANNEL.receive().await;

        // Brief critical section
        ISOTP_BLE_BRIDGE
            .lock()
            .await
            .handle_can_frame(can_message.id, &can_message.data)
            .await;

        // blink led
        led::blink().await;
    }
}

#[embassy_executor::task]
pub async fn isotp_ble_bridge_ble_rx_task() {
    info!("BLE IsoTP bridge BLE task started");

    loop {
        let parsed_message = ISOTP_BLE_CHANNEL.receive().await;

        // Brief critical section
        match ISOTP_BLE_BRIDGE
            .lock()
            .await
            .handle_ble_message(&parsed_message)
            .await
        {
            Ok(_) => (),
            Err(e) => error!("Error handling BLE message: {:?}", e),
        }

        // blink led
        led::blink().await;
    }
}

// Helper functions to send messages to the IsoTP task
pub async fn handle_ble_message(message: ParsedBleMessage) {
    ISOTP_BLE_CHANNEL.send(message).await;
}

pub async fn handle_can_message(message: CanMessage) {
    ISOTP_CAN_CHANNEL.send(message).await;
}
