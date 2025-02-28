use crate::ble_protocol::*;
use crate::isotp_handler::IsotpHandler;
use defmt::{info, Format};
use heapless::FnvIndexMap;

/// Error type for message parsing
#[derive(Debug, Format)]
pub enum ManagerError {
    FailedToInsertFilter,
    FilterAlreadyExists,
    InvalidOffset,
    FilterNotFound,
    FailedToSendMessage,
}

const MAX_HANDLERS: usize = 6;

pub struct IsoTpManager {
    handlers: FnvIndexMap<u32, IsotpHandler, MAX_HANDLERS>,
    tx_buffer: heapless::Vec<u8, 4096>,
}

impl IsoTpManager {
    pub fn new() -> Self {
        Self {
            handlers: FnvIndexMap::new(),
            tx_buffer: heapless::Vec::new(),
        }
    }

    pub async fn handle_message(&mut self, parsed: &ParsedMessage<'_>) -> Result<(), ManagerError> {
        match parsed {
            ParsedMessage::UploadIsotpChunk(upload_chunk_command) => {
                let offset = upload_chunk_command.offset;
                let chunk_length = upload_chunk_command.chunk_length;
                let chunk = upload_chunk_command.chunk;

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
            ParsedMessage::SendIsotpBuffer(send_isotp_buffer_command) => {
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
                let filter_index = self.handlers.iter().position(|(key, handler)| {
                    handler.request_arbitration_id == request_arbitration_id
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
            ParsedMessage::StartPeriodicMessage(_start_periodic_message_command) => {
                todo!()
            }
            ParsedMessage::StopPeriodicMessage(_stop_periodic_message_command) => {
                todo!()
            }
            ParsedMessage::ConfigureIsotpFilter(configure_filter_command) => {
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
}
