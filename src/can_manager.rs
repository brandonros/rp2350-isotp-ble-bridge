use defmt::{debug, error, info, Format};
use embassy_rp::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use portable_atomic::{AtomicPtr, Ordering};

use crate::{channels::CAN_CHANNEL, isotp_ble_bridge};

#[derive(Debug, Format)]
pub struct CanMessage {
    pub id: u32,
    pub data: heapless::Vec<u8, 8>,
}

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

const MAX_FILTERS: usize = 8;
static mut FILTER_IDS: [u32; MAX_FILTERS] = [0; MAX_FILTERS];
static mut FILTER_COUNT: u8 = 0;

// Add this near the other static declarations
static RESET_REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Modified callback with direct comparison
extern "C" fn can_callback(
    _cd: *mut can2040_rs::can2040,
    notify: u32,
    msg: *mut can2040_rs::can2040_msg,
) {
    if notify == can2040_rs::notify::RX {
        // Safety: msg is valid when notification is RX
        if msg.is_null() {
            error!("[can] CAN message is null");
            return;
        }
        let msg = unsafe { &*msg };

        // check if message matches any of our filters
        let filter_count = unsafe { FILTER_COUNT };
        let mut found = false;
        for i in 0..filter_count as usize {
            if msg.id == unsafe { FILTER_IDS[i] } {
                found = true;
                break;
            }
        }

        // drop message if it does not match our filters
        if !found {
            return;
        }

        // log
        let frame_data = unsafe { msg.__bindgen_anon_1.data };
        info!(
            "[can] CAN message received id = {:x} dlc = {:x} data = {:02x}",
            msg.id, msg.dlc, frame_data
        );

        // send message to isotp_ble_bridge
        let mut data = heapless::Vec::new();
        if data
            .extend_from_slice(&frame_data[..(msg.dlc as usize)])
            .is_ok()
        {
            match CAN_RX_QUEUE.try_send(CanMessage { id: msg.id, data }) {
                Ok(_) => (),
                Err(e) => error!(
                    "[can] Failed to send CAN message to isotp_ble_bridge: {}",
                    e
                ),
            }
        }
    } else if notify == can2040_rs::notify::TX {
        info!("[can] CAN message sent");
    } else if notify & can2040_rs::notify::ERROR != 0 {
        // Extract error code by masking out the ERROR notification bit
        let error_code = notify & !can2040_rs::notify::ERROR;
        error!("[can] CAN error: code={:#x}", error_code);
        // Signal that a reset is needed
        RESET_REQUESTED.signal(());
    } else {
        debug!("[can] can_callback: unknown notify: {}", notify);
    }
}

#[embassy_executor::task]
pub async fn can_tx_channel_task() {
    info!("[can] CAN task started");

    loop {
        // Wait for the next message
        let can_message = CAN_CHANNEL.receive().await;

        info!(
            "[can] sending CAN message to {:x} {:02x}",
            can_message.id, can_message.data
        );

        if can_message.data.len() != 8 {
            error!("[can] CAN message data is not 8 bytes");
            continue;
        }

        // Load the pointer once
        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);

        if can_ptr.is_null() {
            error!("[can] CAN instance not initialized");
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
            error!("[can] CAN tx buffer is full");
            continue;
        }

        // send
        match unsafe { (*can_ptr).transmit(&mut msg) } {
            Ok(_) => debug!("[can] CAN message sent successfully"),
            Err(e) => error!("[can] Failed to send CAN message: {}", e),
        }
    }
}

// Replace the old send_message with an async version
pub async fn send_message(id: u32, data: &[u8]) -> bool {
    let mut vec = heapless::Vec::new();
    match vec.extend_from_slice(data) {
        Ok(_) => {
            // Send message to CAN task
            CAN_CHANNEL.send(CanMessage { id, data: vec }).await;
            true
        }
        Err(_) => {
            error!("[can] Data too large for CAN message");
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

const PIO_NUM: u32 = 2;
const BITRATE: u32 = 500_000;
const GPIO_RX: u32 = 10;
const GPIO_TX: u32 = 11;

pub fn init_can() {
    use embassy_rp::interrupt::InterruptExt;
    use embassy_rp::interrupt::Priority;

    unsafe { cortex_m::peripheral::NVIC::unmask(embassy_rp::interrupt::PIO2_IRQ_0) };
    embassy_rp::interrupt::PIO2_IRQ_0.set_priority(Priority::P2);

    // Create CAN instance in static storage to ensure it lives for the program duration
    static mut CAN: Option<can2040_rs::Can2040> = None;

    // Safety: This is only called once during initialization
    let can = unsafe {
        CAN = Some(can2040_rs::Can2040::new(PIO_NUM));
        CAN.as_mut().unwrap()
    };

    can.setup();
    can.set_callback(Some(can_callback));
    let can_ptr = can as *mut _;
    init_instance(can_ptr);

    let sys_clock = embassy_rp::clocks::clk_sys_freq(); // 150_000_000
    can.start(sys_clock, BITRATE, GPIO_RX, GPIO_TX);
}

#[embassy_executor::task]
pub async fn can_stats_task() {
    loop {
        let stats = get_statistics().unwrap();
        info!(
            "[can] stats: tx {:?}, tx_attempt {:?}, parse_error {:?}, rx {:?}",
            stats.tx_total, stats.tx_attempt, stats.parse_error, stats.rx_total
        );
        Timer::after(Duration::from_millis(1000)).await;
    }
}

// New task to process the ring buffer
#[embassy_executor::task]
pub async fn can_rx_channel_task() {
    loop {
        let message = CAN_RX_QUEUE.receive().await;
        isotp_ble_bridge::handle_can_message(message).await;
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

// Add new task to handle CAN reset requests
#[embassy_executor::task]
pub async fn can_reset_task() {
    loop {
        // Wait for reset signal
        RESET_REQUESTED.wait().await;
        error!("[can] Reset requested due to CAN error");

        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
        if !can_ptr.is_null() {
            unsafe { (*can_ptr).stop() };
            let sys_clock = embassy_rp::clocks::clk_sys_freq(); // 150_000_000
            unsafe { (*can_ptr).start(sys_clock, BITRATE, GPIO_RX, GPIO_TX) };
        }
    }
}
