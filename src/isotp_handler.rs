use core::sync::atomic::{AtomicU8, Ordering};
use defmt::{debug, error, info};
use embassy_time::{Duration, Timer};
use heapless::Vec;

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

pub struct IsotpHandler {
    rx_buffer: Vec<u8, 4096>,
    tx_buffer: Vec<u8, 4096>,
    tx_index: AtomicU8,
    st_min: AtomicU8,
    block_size: AtomicU8,
}

impl IsotpHandler {
    pub fn new() -> Self {
        Self {
            rx_buffer: Vec::new(),
            tx_buffer: Vec::new(),
            tx_index: AtomicU8::new(0),
            st_min: AtomicU8::new(DEFAULT_ST_MIN),
            block_size: AtomicU8::new(DEFAULT_BLOCK_SIZE),
        }
    }

    pub fn handle_received_frame(&mut self, id: u32, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let frame_type = data[0] >> 4;
        match frame_type {
            0 => self.handle_single_frame(data),
            1 => self.handle_first_frame(data),
            2 => self.handle_consecutive_frame(data),
            3 => self.handle_flow_control(data),
            _ => error!("Unknown frame type: {}", frame_type),
        }
    }

    pub async fn send_message(&mut self, id: u32, data: &[u8]) -> bool {
        if data.len() <= SF_DL_MAX {
            self.send_single_frame(id, data)
        } else {
            self.send_multi_frame(id, data).await
        }
    }

    fn send_single_frame(&self, id: u32, data: &[u8]) -> bool {
        let mut frame = Vec::<u8, 8>::new();
        frame
            .extend_from_slice(&[SINGLE_FRAME | (data.len() as u8)])
            .unwrap();
        frame.extend_from_slice(data).unwrap();
        can_manager::send_message(id, &frame)
    }

    async fn send_multi_frame(&mut self, id: u32, data: &[u8]) -> bool {
        // Send First Frame
        let mut frame = Vec::<u8, 8>::new();
        let length = data.len();
        frame
            .extend_from_slice(&[FIRST_FRAME | ((length >> 8) as u8), length as u8])
            .unwrap();
        frame.extend_from_slice(&data[0..6]).unwrap();

        if !can_manager::send_message(id, &frame) {
            return false;
        }

        // Store remaining data in tx buffer
        self.tx_buffer.clear();
        self.tx_buffer.extend_from_slice(&data[6..]).unwrap();
        self.tx_index.store(1, Ordering::Release);

        // Wait for flow control and send consecutive frames
        // TODO: Implement flow control handling and consecutive frame sending
        true
    }

    fn handle_single_frame(&mut self, data: &[u8]) {
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
    }

    fn handle_first_frame(&mut self, _data: &[u8]) {
        // TODO: Implement First Frame handling
    }

    fn handle_consecutive_frame(&mut self, _data: &[u8]) {
        // TODO: Implement Consecutive Frame handling
    }

    fn handle_flow_control(&mut self, data: &[u8]) {
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
