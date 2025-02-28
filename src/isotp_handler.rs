use core::sync::atomic::{AtomicU8, Ordering};
use defmt::{debug, error, info};
use heapless::Vec;
use portable_atomic::AtomicU16;

use crate::ble_protocol::IsoTpMessage;
use crate::ble_server::{self};
use crate::can_manager;

// ISO-15765 constants
const SF_DL_MAX: usize = 7; // Single Frame max data length
const FF_DL_MAX: usize = 4095; // First Frame max data length
const CF_DL_MAX: usize = 7; // Consecutive Frame max data length

// Frame types
const SINGLE_FRAME: u8 = 0x00;
const FIRST_FRAME: u8 = 0x10;
const CONSECUTIVE_FRAME: u8 = 0x20;
const FLOW_CONTROL: u8 = 0x30;

// Flow Status
const CONTINUE_TO_SEND: u8 = 0x00;
const WAIT: u8 = 0x01;
const OVERFLOW: u8 = 0x02;

// Default timing parameters (in milliseconds)
const DEFAULT_ST_MIN: u8 = 0x0A; // 10ms
const DEFAULT_BLOCK_SIZE: u8 = 0x00; // Send all frames

const DEFAULT_TX_PAD_BYTE: u8 = 0x55;

pub struct IsotpHandler {
    pub request_arbitration_id: u32,
    pub reply_arbitration_id: u32,
    rx_buffer: Vec<u8, 4096>,
    tx_buffer: Vec<u8, 4096>,
    tx_index: AtomicU8,
    st_min: AtomicU8,
    block_size: AtomicU8,
    expected_sequence_number: AtomicU8,
    remaining_block_size: AtomicU8,
    expected_length: AtomicU16,
}

impl IsotpHandler {
    pub fn new(request_arbitration_id: u32, reply_arbitration_id: u32) -> Self {
        Self {
            request_arbitration_id,
            reply_arbitration_id,
            rx_buffer: Vec::new(),
            tx_buffer: Vec::new(),
            tx_index: AtomicU8::new(0),
            st_min: AtomicU8::new(DEFAULT_ST_MIN),
            block_size: AtomicU8::new(DEFAULT_BLOCK_SIZE),
            expected_sequence_number: AtomicU8::new(0),
            remaining_block_size: AtomicU8::new(0),
            expected_length: AtomicU16::new(0),
        }
    }

    pub async fn handle_received_can_frame(&mut self, id: u32, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let frame_type = data[0] >> 4;
        match frame_type {
            0 => self.handle_single_frame(id, data).await,
            1 => self.handle_first_frame(id, data).await,
            2 => self.handle_consecutive_frame(id, data).await,
            3 => self.handle_flow_control(id, data).await,
            _ => error!("Unknown frame type: {}", frame_type),
        }
    }

    pub async fn send_message(&mut self, id: u32, data: &[u8]) -> bool {
        if data.len() <= SF_DL_MAX {
            self.send_single_frame(id, data).await
        } else {
            self.send_multi_frame(id, data).await
        }
    }

    fn pad_frame(frame: &mut Vec<u8, 8>) {
        while frame.len() < 8 {
            frame.extend_from_slice(&[DEFAULT_TX_PAD_BYTE]).unwrap();
        }
    }

    async fn send_single_frame(&self, id: u32, data: &[u8]) -> bool {
        let mut frame = Vec::<u8, 8>::new();
        frame
            .extend_from_slice(&[SINGLE_FRAME | (data.len() as u8)])
            .unwrap();
        frame.extend_from_slice(data).unwrap();
        Self::pad_frame(&mut frame);
        can_manager::send_message(id, &frame).await
    }

    async fn send_multi_frame(&mut self, id: u32, data: &[u8]) -> bool {
        // Send First Frame
        let mut frame = Vec::<u8, 8>::new();
        let length = data.len();
        frame
            .extend_from_slice(&[FIRST_FRAME | ((length >> 8) as u8), length as u8])
            .unwrap();
        frame.extend_from_slice(&data[0..6]).unwrap();
        // First frame is already 8 bytes, no padding needed

        if !can_manager::send_message(id, &frame).await {
            return false;
        }

        // Store remaining data in tx buffer
        self.tx_buffer.clear();
        self.tx_buffer.extend_from_slice(&data[6..]).unwrap();
        self.tx_index.store(1, Ordering::Release);

        let mut sequence_number: u8 = 1;
        let mut data_index = 6;

        while data_index < data.len() {
            // Wait for ST_MIN
            let st_min = self.st_min.load(Ordering::Acquire);
            if st_min > 0 {
                embassy_time::Timer::after(embassy_time::Duration::from_millis(st_min as u64))
                    .await;
            }

            let mut frame = Vec::<u8, 8>::new();
            frame
                .push(CONSECUTIVE_FRAME | (sequence_number & 0x0F))
                .unwrap();

            let remaining = data.len() - data_index;
            let chunk_size = remaining.min(CF_DL_MAX);
            frame
                .extend_from_slice(&data[data_index..data_index + chunk_size])
                .unwrap();
            Self::pad_frame(&mut frame);

            if !can_manager::send_message(id, &frame).await {
                return false;
            }

            data_index += chunk_size;
            sequence_number = if sequence_number == 0x0F {
                0
            } else {
                sequence_number + 1
            };

            let block_size = self.block_size.load(Ordering::Acquire);
            if block_size > 0 {
                let mut remaining = self.remaining_block_size.load(Ordering::Acquire);
                remaining -= 1;
                if remaining == 0 {
                    // Wait for next Flow Control frame
                    // Note: In a complete implementation, you would want to add timeout handling here
                    self.remaining_block_size
                        .store(block_size, Ordering::Release);
                }
            }
        }

        true
    }

    async fn handle_single_frame(&mut self, _id: u32, data: &[u8]) {
        let length = data[0] & 0x0F;
        if length as usize > data.len() - 1 {
            error!("Invalid SF length");
            return;
        }

        self.rx_buffer.clear();
        self.rx_buffer
            .extend_from_slice(&data[1..=length as usize])
            .unwrap();

        info!("Received complete message: {:02x}", self.rx_buffer);

        // Send structured response to BLE client
        let message = IsoTpMessage {
            request_arbitration_id: self.request_arbitration_id,
            reply_arbitration_id: self.reply_arbitration_id,
            pdu: self.rx_buffer.clone(),
        };
        ble_server::send_isotp_response(message).await;
    }

    async fn handle_first_frame(&mut self, id: u32, data: &[u8]) {
        if data.len() < 2 {
            error!("Invalid FF length");
            return;
        }

        let length = (((data[0] & 0x0F) as u16) << 8) | (data[1] as u16);
        if length > FF_DL_MAX as u16 {
            error!("FF length too large: {}", length);
            return;
        }

        self.rx_buffer.clear();
        self.rx_buffer.extend_from_slice(&data[2..]).unwrap();
        self.expected_length.store(length, Ordering::Release);
        self.expected_sequence_number.store(1, Ordering::Release);

        // Send Flow Control frame
        let mut fc_frame = heapless::Vec::<u8, 8>::new();
        fc_frame
            .extend_from_slice(&[
                FLOW_CONTROL | CONTINUE_TO_SEND,
                DEFAULT_BLOCK_SIZE,
                DEFAULT_ST_MIN,
            ])
            .unwrap();
        Self::pad_frame(&mut fc_frame);

        // Send flow control frame asynchronously
        can_manager::send_message(id, &fc_frame).await;
    }

    async fn handle_consecutive_frame(&mut self, _id: u32, data: &[u8]) {
        if data.len() < 2 {
            error!("Invalid CF length");
            return;
        }

        let sequence_number = data[0] & 0x0F;
        let expected = self.expected_sequence_number.load(Ordering::Acquire);

        if sequence_number != expected {
            error!(
                "Unexpected sequence number. Expected: {}, got: {}",
                expected, sequence_number
            );
            return;
        }

        self.rx_buffer.extend_from_slice(&data[1..]).unwrap();

        let next_sequence = if expected == 0x0F { 0 } else { expected + 1 };
        self.expected_sequence_number
            .store(next_sequence, Ordering::Release);

        let expected_length = self.expected_length.load(Ordering::Acquire) as usize;
        if self.rx_buffer.len() >= expected_length {
            info!(
                "Received complete multi-frame message: {:02x}",
                self.rx_buffer
            );
            self.rx_buffer.truncate(expected_length);

            // Send structured response to BLE client
            let message = IsoTpMessage {
                request_arbitration_id: self.request_arbitration_id,
                reply_arbitration_id: self.reply_arbitration_id,
                pdu: self.rx_buffer.clone(),
            };
            ble_server::send_isotp_response(message).await;
        }
    }

    async fn handle_flow_control(&mut self, _id: u32, data: &[u8]) {
        if data.len() < 3 {
            error!("Invalid FC frame length");
            return;
        }

        let flow_status = data[0] & 0x0F;
        match flow_status {
            CONTINUE_TO_SEND => {
                self.block_size.store(data[1], Ordering::Release);
                self.st_min.store(data[2], Ordering::Release);
            }
            WAIT => {
                debug!("Received WAIT flow status");
            }
            OVERFLOW => {
                error!("Received OVERFLOW flow status");
            }
            _ => error!("Invalid flow status: {}", flow_status),
        }
    }
}
