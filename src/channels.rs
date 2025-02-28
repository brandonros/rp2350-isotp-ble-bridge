//! Inter-module communication channels
//! This module centralizes all communication channels between different components

use crate::ble_protocol::{IsoTpMessage, ParsedBleMessage};
use crate::can_manager::CanMessage;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;

/// Channel for BLE responses (ISOTP -> BLE)
pub static BLE_RESPONSE_CHANNEL: Channel<ThreadModeRawMutex, IsoTpMessage, 16> = Channel::new();

/// Channel for CAN messages (CAN Hardware -> ISOTP)
pub static CAN_CHANNEL: Channel<CriticalSectionRawMutex, CanMessage, 16> = Channel::new();

/// Channel for BLE commands (BLE -> ISOTP)
pub static ISOTP_BLE_CHANNEL: Channel<ThreadModeRawMutex, ParsedBleMessage, 16> = Channel::new();

/// Channel for CAN messages to be processed by ISOTP (CAN -> ISOTP)
pub static ISOTP_CAN_CHANNEL: Channel<ThreadModeRawMutex, CanMessage, 16> = Channel::new();
