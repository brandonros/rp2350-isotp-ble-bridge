use defmt::{debug, error, info};
use embassy_rp::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use portable_atomic::{AtomicPtr, Ordering};

#[derive(Debug)]
pub struct CanMessage {
    pub id: u32,
    pub data: heapless::Vec<u8, 8>,
}

// Message type for the CAN task
#[derive(Debug)]
pub enum CanChannelMessage {
    Send(CanMessage),
    // Add other message types as needed
}

// Channel for communicating with the CAN task
pub static CAN_CHANNEL: Channel<CriticalSectionRawMutex, CanChannelMessage, 16> = Channel::new();

static CAN_INSTANCE: AtomicPtr<can2040_rs::Can2040> = AtomicPtr::new(core::ptr::null_mut());

pub struct CanInterruptHandler;

impl interrupt::typelevel::Handler<interrupt::typelevel::PIO2_IRQ_0> for CanInterruptHandler {
    unsafe fn on_interrupt() {
        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
        if !can_ptr.is_null() {
            (*can_ptr).handle_interrupt();
        }
    }
}

// Fixed-size ring buffer for incoming CAN messages
const RING_BUFFER_SIZE: usize = 32;
static CAN_RX_QUEUE: Channel<CriticalSectionRawMutex, CanMessage, RING_BUFFER_SIZE> =
    Channel::new();

// For 4-6 IDs, we can just use a small fixed array
const MAX_FILTERS: usize = 8; // Round up to next power of 2 for good measure
static mut FILTER_IDS: [u32; MAX_FILTERS] = [0; MAX_FILTERS];
static mut FILTER_COUNT: u8 = 0;

// Modified callback with direct comparison
extern "C" fn can_callback(
    _cd: *mut can2040_rs::can2040,
    notify: u32,
    msg: *mut can2040_rs::can2040_msg,
) {
    if notify == can2040_rs::notify::RX {
        // Safety: msg is valid when notification is RX
        let msg = unsafe { &*msg };

        // Direct comparison against our small set of IDs
        // Safety: We're only reading these values, and they're only modified during init
        let count = unsafe { FILTER_COUNT };
        let ids = unsafe { &FILTER_IDS };

        // For such a small set, a simple loop is fastest
        let mut found = false;
        for i in 0..count as usize {
            if msg.id == ids[i] {
                found = true;
                break;
            }
        }

        // drop message if it does not match our filters
        if !found {
            return;
        }

        // Rest of the handler...
        let mut data = heapless::Vec::new();
        let frame_data = unsafe { msg.__bindgen_anon_1.data };
        if data
            .extend_from_slice(&frame_data[..(msg.dlc as usize)])
            .is_ok()
        {
            let _ = CAN_RX_QUEUE.try_send(CanMessage { id: msg.id, data });
        }
    } else if notify == can2040_rs::notify::TX {
        info!("CAN message sent");
    } else if notify == can2040_rs::notify::ERROR {
        info!("CAN error");
    } else {
        debug!("can_callback: unknown notify: {}", notify);
    }
}

#[embassy_executor::task]
pub async fn can_channel_task() {
    info!("CAN task started");

    loop {
        // Wait for the next message
        match CAN_CHANNEL.receive().await {
            CanChannelMessage::Send(can_message) => {
                // Load the pointer once
                let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);

                if can_ptr.is_null() {
                    error!("CAN instance not initialized");
                    continue;
                }

                // build message
                let mut msg = can2040_rs::can2040_msg::default();
                msg.id = can_message.id;
                msg.dlc = can_message.data.len() as u32;
                for (i, &byte) in can_message.data.iter().enumerate() {
                    unsafe {
                        msg.__bindgen_anon_1.data[i] = byte;
                    }
                }

                // check if we can transmit
                let tx_avail = unsafe { (*can_ptr).check_transmit() };
                if tx_avail <= 0 {
                    error!("CAN tx buffer is full");
                    continue;
                }

                // send
                info!("sending CAN message");
                match unsafe { (*can_ptr).transmit(&mut msg) } {
                    Ok(_) => debug!("CAN message sent successfully"),
                    Err(e) => error!("Failed to send CAN message: {}", e),
                }
            }
        }
    }
}

// Replace the old send_message with an async version
pub async fn send_message(id: u32, data: &[u8]) -> bool {
    let mut vec = heapless::Vec::new();
    match vec.extend_from_slice(data) {
        Ok(_) => {
            // Send message to CAN task
            CAN_CHANNEL
                .send(CanChannelMessage::Send(CanMessage { id, data: vec }))
                .await;
            true
        }
        Err(_) => {
            error!("Data too large for CAN message");
            false
        }
    }
}

pub fn init_instance(can: *mut can2040_rs::Can2040) {
    CAN_INSTANCE.store(can, Ordering::Release);
}

#[allow(dead_code)]
pub fn get_statistics() -> Option<can2040_rs::can2040_stats> {
    let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
    if !can_ptr.is_null() {
        Some(unsafe { (*can_ptr).get_statistics() })
    } else {
        None
    }
}

pub fn init_can(pio_num: u32, gpio_rx: u32, gpio_tx: u32, sys_clock: u32, bitrate: u32) {
    use embassy_rp::interrupt::InterruptExt;
    use embassy_rp::interrupt::Priority;

    unsafe { cortex_m::peripheral::NVIC::unmask(embassy_rp::interrupt::PIO2_IRQ_0) };
    embassy_rp::interrupt::PIO2_IRQ_0.set_priority(Priority::P1);

    let mut can = can2040_rs::Can2040::new(pio_num);
    can.setup();
    can.set_callback(Some(can_callback));
    let can_ptr = &mut can as *mut _;
    init_instance(can_ptr);
    can.start(sys_clock, bitrate, gpio_rx, gpio_tx);
}

// New task to process the ring buffer
#[embassy_executor::task]
pub async fn can_isotp_dispatch_task() {
    use crate::isotp_manager;

    loop {
        let message = CAN_RX_QUEUE.receive().await;
        isotp_manager::handle_can_message(message).await;
    }
}

// Simplified filter registration
pub fn register_isotp_filter(response_id: u32) -> bool {
    critical_section::with(|_| {
        // Safety: We're in a critical section
        unsafe {
            if FILTER_COUNT as usize >= MAX_FILTERS - 1 {
                return false;
            }

            FILTER_IDS[FILTER_COUNT as usize] = response_id;
            FILTER_COUNT += 1;
        }
        true
    })
}
