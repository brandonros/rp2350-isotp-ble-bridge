use core::convert::TryFrom;

/// Error type for message parsing
#[derive(Debug)]
pub enum ParseError {
    InvalidCommand,
    BufferTooSmall,
    InvalidLength,
}

/// Command IDs extracted from the JavaScript code
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    UploadChunk = 0x02,
    TriggerBleSend = 0x03,
    StartPeriodicMessage = 0x04,
    StopPeriodicMessage = 0x05,
    ConfigureFilter = 0x06,
}

impl TryFrom<u8> for CommandId {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x02 => Ok(CommandId::UploadChunk),
            0x03 => Ok(CommandId::TriggerBleSend),
            0x04 => Ok(CommandId::StartPeriodicMessage),
            0x05 => Ok(CommandId::StopPeriodicMessage),
            0x06 => Ok(CommandId::ConfigureFilter),
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
pub struct UploadChunkCommand<'a> {
    // 16-bit offset in original JS
    pub offset: u16,
    // 16-bit length in original JS
    pub chunk_length: u16,
    // Actual chunk data
    pub chunk: &'a [u8],
}

impl<'a> Command for UploadChunkCommand<'a> {
    fn command_id(&self) -> CommandId {
        CommandId::UploadChunk
    }
}

impl<'a> UploadChunkCommand<'a> {
    /// Parse an upload chunk command from a byte buffer
    pub fn parse(buffer: &'a [u8]) -> Result<Self, ParseError> {
        // Need at least 5 bytes: command(1) + offset(2) + length(2)
        if buffer.len() < 5 {
            return Err(ParseError::BufferTooSmall);
        }

        // In JS: command.writeUInt16BE(offset, 0x01)
        let offset = u16::from_be_bytes([buffer[1], buffer[2]]);

        // In JS: command.writeUInt16BE(chunk.length, 0x03)
        let chunk_length = u16::from_be_bytes([buffer[3], buffer[4]]);

        // Validate that buffer contains enough data
        if buffer.len() < 5 + chunk_length as usize {
            return Err(ParseError::BufferTooSmall);
        }

        // In JS: chunk.copy(command, 5)
        let chunk = &buffer[5..5 + chunk_length as usize];

        Ok(Self {
            offset,
            chunk_length,
            chunk,
        })
    }
}

/// Trigger BLE Send Command (0x03)
/// Used to trigger sending of accumulated chunks
#[derive(Debug)]
pub struct TriggerBleSendCommand {
    // Total length of message to send
    pub total_length: u16,
}

impl Command for TriggerBleSendCommand {
    fn command_id(&self) -> CommandId {
        CommandId::TriggerBleSend
    }
}

impl TriggerBleSendCommand {
    /// Parse a trigger BLE send command from a byte buffer
    pub fn parse(buffer: &[u8]) -> Result<Self, ParseError> {
        // Need 3 bytes: command(1) + length(2)
        if buffer.len() < 3 {
            return Err(ParseError::BufferTooSmall);
        }

        // In JS: command.writeUInt16BE(offset, 0x01)
        let total_length = u16::from_be_bytes([buffer[1], buffer[2]]);

        Ok(Self { total_length })
    }
}

/// Start Periodic Message Command (0x04)
/// Used to start sending a message periodically
#[derive(Debug)]
pub struct StartPeriodicMessageCommand<'a> {
    // Periodic message index (8-bit in JS)
    pub periodic_message_index: u8,
    // Interval in milliseconds (16-bit in JS)
    pub interval: u16,
    // Request arbitration ID (32-bit in JS)
    pub request_arbitration_id: u32,
    // Reply arbitration ID (32-bit in JS)
    pub reply_arbitration_id: u32,
    // Number of messages (16-bit in JS)
    pub message_count: u16,
    // Message payloads (each preceded by 16-bit length)
    pub message_data: &'a [u8],
}

impl<'a> Command for StartPeriodicMessageCommand<'a> {
    fn command_id(&self) -> CommandId {
        CommandId::StartPeriodicMessage
    }
}

impl<'a> StartPeriodicMessageCommand<'a> {
    /// Parse a start periodic message command from a byte buffer
    pub fn parse(buffer: &'a [u8]) -> Result<Self, ParseError> {
        // Need at least 14 bytes for header
        // command(1) + index(1) + interval(2) + req_id(4) + reply_id(4) + msg_count(2)
        if buffer.len() < 14 {
            return Err(ParseError::BufferTooSmall);
        }

        let periodic_message_index = buffer[1];
        let interval = u16::from_be_bytes([buffer[2], buffer[3]]);
        let request_arbitration_id =
            u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let reply_arbitration_id =
            u32::from_be_bytes([buffer[8], buffer[9], buffer[10], buffer[11]]);
        let message_count = u16::from_be_bytes([buffer[12], buffer[13]]);

        // Message data starts at offset 14
        let message_data = &buffer[14..];

        Ok(Self {
            periodic_message_index,
            interval,
            request_arbitration_id,
            reply_arbitration_id,
            message_count,
            message_data,
        })
    }

    /// Helper to iterate over the individual messages in the payload
    pub fn iter_messages(&self) -> PeriodicMessageIterator {
        PeriodicMessageIterator {
            data: self.message_data,
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
pub struct ConfigureFilterCommand<'a> {
    // Filter ID
    pub filter_id: u32,
    // Request arbitration ID
    pub request_arbitration_id: u32,
    // Reply arbitration ID
    pub reply_arbitration_id: u32,
    // Filter name (null-terminated string)
    pub name: &'a [u8],
}

impl<'a> Command for ConfigureFilterCommand<'a> {
    fn command_id(&self) -> CommandId {
        CommandId::ConfigureFilter
    }
}

impl<'a> ConfigureFilterCommand<'a> {
    /// Parse a configure filter command from a byte buffer
    pub fn parse(buffer: &'a [u8]) -> Result<Self, ParseError> {
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
            name,
        })
    }
}

/// Message payload with arbitration IDs
/// This represents the format of data messages
#[derive(Debug)]
pub struct IsoTpMessage<'a> {
    pub request_arbitration_id: u32,
    pub reply_arbitration_id: u32,
    pub pdu: &'a [u8],
}

impl<'a> IsoTpMessage<'a> {
    pub fn parse(buffer: &'a [u8]) -> Result<Self, ParseError> {
        // Need at least 8 bytes for the arbitration IDs
        if buffer.len() < 8 {
            return Err(ParseError::BufferTooSmall);
        }

        let request_arbitration_id =
            u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        let reply_arbitration_id = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let pdu = &buffer[8..];

        Ok(Self {
            request_arbitration_id,
            reply_arbitration_id,
            pdu,
        })
    }
}

/// Main message parser
pub struct MessageParser;

impl MessageParser {
    /// Parse a message from a byte buffer
    pub fn parse<'a>(buffer: &'a [u8]) -> Result<ParsedMessage<'a>, ParseError> {
        if buffer.is_empty() {
            return Err(ParseError::BufferTooSmall);
        }

        let command_id = CommandId::try_from(buffer[0])?;

        match command_id {
            CommandId::UploadChunk => {
                let command: UploadChunkCommand<'_> = UploadChunkCommand::parse(buffer)?;
                Ok(ParsedMessage::UploadChunk(command))
            }
            CommandId::TriggerBleSend => {
                let command = TriggerBleSendCommand::parse(buffer)?;
                Ok(ParsedMessage::TriggerBleSend(command))
            }
            CommandId::StartPeriodicMessage => {
                let command = StartPeriodicMessageCommand::parse(buffer)?;
                Ok(ParsedMessage::StartPeriodicMessage(command))
            }
            CommandId::StopPeriodicMessage => {
                let command = StopPeriodicMessageCommand::parse(buffer)?;
                Ok(ParsedMessage::StopPeriodicMessage(command))
            }
            CommandId::ConfigureFilter => {
                let command = ConfigureFilterCommand::parse(buffer)?;
                Ok(ParsedMessage::ConfigureFilter(command))
            }
        }
    }
}

/// Enum containing all possible parsed messages
#[derive(Debug)]
pub enum ParsedMessage<'a> {
    UploadChunk(UploadChunkCommand<'a>),
    TriggerBleSend(TriggerBleSendCommand),
    StartPeriodicMessage(StartPeriodicMessageCommand<'a>),
    StopPeriodicMessage(StopPeriodicMessageCommand),
    ConfigureFilter(ConfigureFilterCommand<'a>),
}

/// Simple message buffer for accumulating chunks
pub struct ChunkBuffer {
    data: [u8; 4096], // Adjust size as needed for your embedded environment
    size: usize,
}

impl ChunkBuffer {
    pub fn new() -> Self {
        Self {
            data: [0; 4096],
            size: 0,
        }
    }

    pub fn clear(&mut self) {
        self.size = 0;
    }

    pub fn add_chunk(&mut self, offset: u16, chunk: &[u8]) -> Result<(), ParseError> {
        let offset = offset as usize;

        // Check if chunk fits
        if offset + chunk.len() > self.data.len() {
            return Err(ParseError::BufferTooSmall);
        }

        // Copy chunk to buffer
        self.data[offset..offset + chunk.len()].copy_from_slice(chunk);

        // Update size if needed
        if offset + chunk.len() > self.size {
            self.size = offset + chunk.len();
        }

        Ok(())
    }

    pub fn get_message(&self) -> &[u8] {
        &self.data[0..self.size]
    }
}
