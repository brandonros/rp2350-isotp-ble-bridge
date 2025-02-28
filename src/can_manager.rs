use defmt::{debug, error, info};
use embassy_rp::interrupt;
use portable_atomic::{AtomicPtr, Ordering};

static CAN_INSTANCE: AtomicPtr<can2040_rs::Can2040> = AtomicPtr::new(core::ptr::null_mut());

// Move the interrupt handler to can_manager
pub struct CanInterruptHandler;
impl interrupt::typelevel::Handler<interrupt::typelevel::PIO2_IRQ_0> for CanInterruptHandler {
    unsafe fn on_interrupt() {
        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
        if !can_ptr.is_null() {
            (*can_ptr).handle_interrupt();
        }
    }
}

pub fn send_message(id: u32, data: &[u8]) -> bool {
    // Load the pointer once
    let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);

    if can_ptr.is_null() {
        error!("CAN instance not initialized");
        return false;
    }

    // build message
    let mut msg = can2040_rs::can2040_msg::default();
    msg.id = id; // Standard ID
    msg.dlc = data.len() as u32;
    for i in 0..data.len() {
        unsafe {
            msg.__bindgen_anon_1.data[i] = data[i];
        }
    }

    // check if we can transmit
    let tx_avail = unsafe { (*can_ptr).check_transmit() };
    if tx_avail <= 0 {
        error!("CAN tx buffer is full");
        return false;
    }

    // send
    info!("sending CAN message");
    match unsafe { (*can_ptr).transmit(&mut msg) } {
        Ok(_) => true,
        Err(e) => {
            error!("Failed to send CAN message: {}", e);
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
