use crate::ble_protocol::*;
use crate::isotp_handler::IsotpHandler;
use defmt::Format;
use heapless::FnvIndexMap;

/// Error type for message parsing
#[derive(Debug, Format)]
pub enum ManagerError {
    Todo,
}

const MAX_HANDLERS: usize = 6;

pub struct IsoTpManager {
    handlers: FnvIndexMap<u32, IsotpHandler, MAX_HANDLERS>,
}

impl IsoTpManager {
    pub fn new() -> Self {
        Self {
            handlers: FnvIndexMap::new(),
        }
    }

    pub async fn handle_message(&mut self, parsed: &ParsedMessage<'_>) -> Result<(), ManagerError> {
        match parsed {
            ParsedMessage::UploadIsotpChunk(upload_chunk_command) => todo!(),
            ParsedMessage::SendIsotpBuffer(send_isotp_buffer_command) => todo!(),
            ParsedMessage::StartPeriodicMessage(start_periodic_message_command) => todo!(),
            ParsedMessage::StopPeriodicMessage(stop_periodic_message_command) => todo!(),
            ParsedMessage::ConfigureIsotpFilter(configure_filter_command) => todo!(),
        }
    }
}
