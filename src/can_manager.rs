use defmt::{debug, error, info};
use embassy_rp::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use portable_atomic::{AtomicPtr, Ordering};

// Message type for the CAN task
#[derive(Debug)]
pub enum CanMessage {
    Send { id: u32, data: heapless::Vec<u8, 8> },
    // Add other message types as needed
}

// Channel for communicating with the CAN task
pub static CAN_CHANNEL: Channel<CriticalSectionRawMutex, CanMessage, 16> = Channel::new();

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

#[embassy_executor::task]
pub async fn can_task() {
    info!("CAN task started");

    loop {
        // Wait for the next message
        match CAN_CHANNEL.receive().await {
            CanMessage::Send { id, data } => {
                // Load the pointer once
                let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);

                if can_ptr.is_null() {
                    error!("CAN instance not initialized");
                    continue;
                }

                // build message
                let mut msg = can2040_rs::can2040_msg::default();
                msg.id = id;
                msg.dlc = data.len() as u32;
                for (i, &byte) in data.iter().enumerate() {
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
            CAN_CHANNEL.send(CanMessage::Send { id, data: vec }).await;
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

pub fn get_statistics() -> Option<can2040_rs::can2040_stats> {
    let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
    if !can_ptr.is_null() {
        Some(unsafe { (*can_ptr).get_statistics() })
    } else {
        None
    }
}

// Add these new functions and the callback:
extern "C" fn can_callback(
    _cd: *mut can2040_rs::can2040,
    notify: u32,
    msg: *mut can2040_rs::can2040_msg,
) {
    if notify == can2040_rs::notify::RX {
        // Safety: msg is valid when notification is RX
        let msg = unsafe { &*msg };
        let data = unsafe { msg.__bindgen_anon_1.data };
        info!(
            "CAN message received: ID: {}, DLC: {} Data: {:02x}",
            msg.id, msg.dlc, data
        );
    } else if notify == can2040_rs::notify::TX {
        info!("CAN message sent");
    } else if notify == can2040_rs::notify::ERROR {
        info!("CAN error");
    } else {
        debug!("can_callback: unknown notify: {}", notify);
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
