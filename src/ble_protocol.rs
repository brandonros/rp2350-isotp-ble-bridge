use core::convert::TryFrom;

use defmt::Format;

/// Error type for message parsing
#[derive(Debug, Format)]
pub enum ParseError {
    InvalidCommand,
    BufferTooSmall,
}

/// Command IDs extracted from the JavaScript code
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    UploadIsotpChunk = 0x02,
    SendIsotpBuffer = 0x03,
    StartPeriodicMessage = 0x04,
    StopPeriodicMessage = 0x05,
    ConfigureIsotpFilter = 0x06,
}

impl TryFrom<u8> for CommandId {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x02 => Ok(CommandId::UploadIsotpChunk),
            0x03 => Ok(CommandId::SendIsotpBuffer),
            0x04 => Ok(CommandId::StartPeriodicMessage),
            0x05 => Ok(CommandId::StopPeriodicMessage),
            0x06 => Ok(CommandId::ConfigureIsotpFilter),
            _ => Err(ParseError::InvalidCommand),
        }
    }
}

/// Common trait for all command messages
pub trait Command {
    fn command_id(&self) -> CommandId;
}

/// Upload Chunk Command (0x02)
/// Used to upload chunks of a large message
#[derive(Debug)]
pub struct UploadChunkCommand {
    pub offset: u16,
    pub chunk_length: u16,
    pub chunk: heapless::Vec<u8, 512>,
}

impl Command for UploadChunkCommand {
    fn command_id(&self) -> CommandId {
        CommandId::UploadIsotpChunk
    }
}

impl UploadChunkCommand {
    /// Parse an upload chunk command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need at least 5 bytes: command(1) + offset(2) + length(2)
        if buffer.len() < 5 {
            return Err(ParseError::BufferTooSmall);
        }

        let offset = u16::from_be_bytes([buffer[1], buffer[2]]);

        let chunk_length = u16::from_be_bytes([buffer[3], buffer[4]]);

        // Validate that buffer contains enough data
        if buffer.len() < 5 + chunk_length as usize {
            return Err(ParseError::BufferTooSmall);
        }

        let chunk = &buffer[5..5 + chunk_length as usize];

        Ok(Self {
            offset,
            chunk_length,
            chunk: heapless::Vec::from_slice(chunk).unwrap(),
        })
    }
}

/// Trigger BLE Send Command (0x03)
/// Used to trigger sending of accumulated chunks
#[derive(Debug)]
pub struct SendIsotpBufferCommand {
    // Total length of message to send
    pub total_length: u16,
}

impl Command for SendIsotpBufferCommand {
    fn command_id(&self) -> CommandId {
        CommandId::SendIsotpBuffer
    }
}

impl SendIsotpBufferCommand {
    /// Parse a trigger BLE send command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need 3 bytes: command(1) + length(2)
        if buffer.len() < 3 {
            return Err(ParseError::BufferTooSmall);
        }

        let total_length = u16::from_be_bytes([buffer[1], buffer[2]]);

        Ok(Self { total_length })
    }
}

/// Start Periodic Message Command (0x04)
/// Used to start sending a message periodically
#[allow(dead_code)]
#[derive(Debug)]
pub struct StartPeriodicMessageCommand {
    pub periodic_message_index: u8,
    pub interval_ms: u16,
    pub request_arbitration_id: u32,
    pub reply_arbitration_id: u32,
    pub message_count: u16,
    pub message_data: heapless::Vec<u8, 512>,
}

impl Command for StartPeriodicMessageCommand {
    fn command_id(&self) -> CommandId {
        CommandId::StartPeriodicMessage
    }
}

#[allow(dead_code)]
impl StartPeriodicMessageCommand {
    /// Parse a start periodic message command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need at least 14 bytes for header
        // command(1) + index(1) + interval(2) + req_id(4) + reply_id(4) + msg_count(2)
        if buffer.len() < 14 {
            return Err(ParseError::BufferTooSmall);
        }

        let periodic_message_index = buffer[1];
        let interval_ms = u16::from_be_bytes([buffer[2], buffer[3]]);
        let request_arbitration_id =
            u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let reply_arbitration_id =
            u32::from_be_bytes([buffer[8], buffer[9], buffer[10], buffer[11]]);
        let message_count = u16::from_be_bytes([buffer[12], buffer[13]]);

        // Message data starts at offset 14
        let message_data = &buffer[14..];

        Ok(Self {
            periodic_message_index,
            interval_ms,
            request_arbitration_id,
            reply_arbitration_id,
            message_count,
            message_data: heapless::Vec::from_slice(message_data).unwrap(),
        })
    }

    /// Helper to iterate over the individual messages in the payload
    pub fn iter_messages(&self) -> PeriodicMessageIterator {
        PeriodicMessageIterator {
            data: self.message_data.as_slice(),
            offset: 0,
        }
    }
}

/// Iterator for periodic messages in a StartPeriodicMessageCommand
pub struct PeriodicMessageIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for PeriodicMessageIterator<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset + 2 > self.data.len() {
            return None;
        }

        // Get message length (16-bit BE)
        let length =
            u16::from_be_bytes([self.data[self.offset], self.data[self.offset + 1]]) as usize;

        // Check if we have enough data
        if self.offset + 2 + length > self.data.len() {
            return None;
        }

        // Get message slice
        let message = &self.data[self.offset + 2..self.offset + 2 + length];

        // Update offset for next iteration
        self.offset += 2 + length;

        Some(message)
    }
}

/// Stop Periodic Message Command (0x05)
/// Used to stop a periodic message
#[derive(Debug)]
#[allow(dead_code)]
pub struct StopPeriodicMessageCommand {
    // Periodic message index to stop
    pub periodic_message_index: u8,
    // Request arbitration ID
    pub request_arbitration_id: u32,
    // Reply arbitration ID
    pub reply_arbitration_id: u32,
}

impl Command for StopPeriodicMessageCommand {
    fn command_id(&self) -> CommandId {
        CommandId::StopPeriodicMessage
    }
}

impl StopPeriodicMessageCommand {
    /// Parse a stop periodic message command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need 10 bytes: command(1) + index(1) + req_id(4) + reply_id(4)
        if buffer.len() < 10 {
            return Err(ParseError::BufferTooSmall);
        }

        let periodic_message_index = buffer[1];
        let request_arbitration_id =
            u32::from_be_bytes([buffer[2], buffer[3], buffer[4], buffer[5]]);
        let reply_arbitration_id = u32::from_be_bytes([buffer[6], buffer[7], buffer[8], buffer[9]]);

        Ok(Self {
            periodic_message_index,
            request_arbitration_id,
            reply_arbitration_id,
        })
    }
}

/// Configure Filter Command (0x06)
/// Used to configure a message filter
#[derive(Debug)]
pub struct ConfigureFilterCommand {
    // Filter ID
    pub filter_id: u32,
    // Request arbitration ID
    pub request_arbitration_id: u32,
    // Reply arbitration ID
    pub reply_arbitration_id: u32,
    // Filter name (null-terminated string)
    pub name: heapless::Vec<u8, 32>,
}

impl Command for ConfigureFilterCommand {
    fn command_id(&self) -> CommandId {
        CommandId::ConfigureIsotpFilter
    }
}

impl ConfigureFilterCommand {
    /// Parse a configure filter command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need at least 13 bytes: command(1) + filter_id(4) + req_id(4) + reply_id(4) + name_len(4)
        if buffer.len() < 17 {
            return Err(ParseError::BufferTooSmall);
        }

        let filter_id = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]);
        let request_arbitration_id =
            u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]);
        let reply_arbitration_id =
            u32::from_be_bytes([buffer[9], buffer[10], buffer[11], buffer[12]]);
        let name_len =
            u32::from_be_bytes([buffer[13], buffer[14], buffer[15], buffer[16]]) as usize;

        // Validate that buffer contains the full name
        if buffer.len() < 17 + name_len {
            return Err(ParseError::BufferTooSmall);
        }

        let name = &buffer[17..17 + name_len];

        Ok(Self {
            filter_id,
            request_arbitration_id,
            reply_arbitration_id,
            name: heapless::Vec::from_slice(name).unwrap(),
        })
    }
}

/// Message payload with arbitration IDs
/// This represents the format of data messages
#[derive(Debug)]
pub struct IsoTpMessage {
    pub request_arbitration_id: u32,
    pub reply_arbitration_id: u32,
    pub pdu: heapless::Vec<u8, 4096>,
}

/// Main message parser
pub struct BleMessageParser;

impl BleMessageParser {
    /// Parse a message from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<ParsedBleMessage, ParseError> {
        if buffer.is_empty() {
            return Err(ParseError::BufferTooSmall);
        }

        let command_id = CommandId::try_from(buffer[0])?;

        match command_id {
            CommandId::UploadIsotpChunk => {
                let command = UploadChunkCommand::parse(buffer)?;
                Ok(ParsedBleMessage::UploadIsotpChunk(command))
            }
            CommandId::SendIsotpBuffer => {
                let command = SendIsotpBufferCommand::parse(buffer)?;
                Ok(ParsedBleMessage::SendIsotpBuffer(command))
            }
            CommandId::StartPeriodicMessage => {
                let command = StartPeriodicMessageCommand::parse(buffer)?;
                Ok(ParsedBleMessage::StartPeriodicMessage(command))
            }
            CommandId::StopPeriodicMessage => {
                let command = StopPeriodicMessageCommand::parse(buffer)?;
                Ok(ParsedBleMessage::StopPeriodicMessage(command))
            }
            CommandId::ConfigureIsotpFilter => {
                let command = ConfigureFilterCommand::parse(buffer)?;
                Ok(ParsedBleMessage::ConfigureIsotpFilter(command))
            }
        }
    }
}

/// Enum containing all possible parsed messages
#[derive(Debug)]
pub enum ParsedBleMessage {
    UploadIsotpChunk(UploadChunkCommand),
    SendIsotpBuffer(SendIsotpBufferCommand),
    StartPeriodicMessage(StartPeriodicMessageCommand),
    StopPeriodicMessage(StopPeriodicMessageCommand),
    ConfigureIsotpFilter(ConfigureFilterCommand),
}
