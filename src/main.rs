#![no_std]
#![no_main]

mod ble_bas_peripheral;
mod ble_protocol;
mod isotp_handler;

use bt_hci::controller::ExternalController;
use cyw43::bluetooth::BtDriver;
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::{debug, error, info, unwrap};
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::interrupt::{InterruptExt as _, Priority};
use embassy_rp::peripherals::{DMA_CH0, PIO0, UART1};
use embassy_rp::pio::{self, Pio};
use embassy_rp::uart::{self};
use embassy_time::{Duration, Timer};
use portable_atomic::{AtomicPtr, Ordering};
use static_cell::StaticCell;
use {defmt_serial as _, panic_probe as _};

// Program metadata for `picotool info`.
#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 4] = [
    embassy_rp::binary_info::rp_program_name!(c"TrouBLE"),
    embassy_rp::binary_info::rp_program_description!(c"BLE Peripheral"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

static CAN_INSTANCE: AtomicPtr<can2040_rs::Can2040> = AtomicPtr::new(core::ptr::null_mut());

// interrupt handlers
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    PIO2_IRQ_0 => CanInterruptHandler;
});

struct CanInterruptHandler;
impl embassy_rp::interrupt::typelevel::Handler<embassy_rp::interrupt::typelevel::PIO2_IRQ_0>
    for CanInterruptHandler
{
    unsafe fn on_interrupt() {
        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);
        if !can_ptr.is_null() {
            (*can_ptr).handle_interrupt();
        }
    }
}

// cyw43 task
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

// blinky task
#[embassy_executor::task]
async fn blinky_task(control: &'static mut cyw43::Control<'static>) {
    loop {
        //control.gpio_set(0, true).await;
        Timer::after(Duration::from_millis(1000)).await;
        //control.gpio_set(0, false).await;
        Timer::after(Duration::from_millis(1000)).await;
    }
}

fn can_send_message(can_ptr: *mut can2040_rs::Can2040, id: u32, data: &[u8]) -> bool {
    // build message
    let mut msg = can2040_rs::can2040_msg::default();
    msg.id = id; // Standard ID
    msg.dlc = data.len() as u32; // 8 bytes of data
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

    // TODO: wait for irq tx event?
}

// can task
#[embassy_executor::task]
async fn can_task() {
    let mut ticker = embassy_time::Ticker::every(Duration::from_millis(1000));

    loop {
        // Load the pointer once per iteration
        let can_ptr = CAN_INSTANCE.load(Ordering::Acquire);

        if !can_ptr.is_null() {
            can_send_message(
                can_ptr,
                0x7e5,
                &[0x02, 0x3e, 0x00, 0x55, 0x55, 0x55, 0x55, 0x55],
            );

            // Get statistics to help with debugging
            let stats = unsafe { (*can_ptr).get_statistics() };
            info!(
                "CAN stats - TX attempts: {}, TX successful: {}, RX: {}, Parse errors: {}",
                stats.tx_attempt, stats.tx_total, stats.rx_total, stats.parse_error
            );
        } else {
            debug!("CAN instance not yet initialized");
        }

        // Use a ticker instead of direct Timer::after for more consistent timing
        ticker.next().await;
    }
}

// ble task
#[embassy_executor::task]
async fn ble_task(bt_device: BtDriver<'static>) {
    let controller: ExternalController<BtDriver<'static>, 10> = ExternalController::new(bt_device);
    ble_bas_peripheral::run::<_, 128>(controller).await;
}

// CAN message callback
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // init peripherals
    let p = embassy_rp::init(Default::default());

    // init uart
    static UART: StaticCell<uart::Uart<'static, UART1, uart::Blocking>> = StaticCell::new();
    let uart1 = UART.init(uart::Uart::new_blocking(
        p.UART1,
        p.PIN_4, // tx, blue, goes to rx
        p.PIN_5, // rx, white, goes to tx
        uart::Config::default(),
    ));

    // init defmt serial
    defmt_serial::defmt_serial(uart1);

    // init cyw43
    let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
    let btfw = include_bytes!("../cyw43-firmware/43439A0_btfw.bin");
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );
    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (_net_device, bt_device, mut control, runner) =
        cyw43::new_with_bluetooth(state, pwr, spi, fw, btfw).await;
    unwrap!(spawner.spawn(cyw43_task(runner)));
    control.init(clm).await;

    // init blinky task
    static CONTROL: StaticCell<cyw43::Control<'static>> = StaticCell::new();
    let control = CONTROL.init(control);
    unwrap!(spawner.spawn(blinky_task(control)));

    // init ble peripheral
    unwrap!(spawner.spawn(ble_task(bt_device)));

    // sleep 1s to allow cyw43 to boot
    Timer::after(Duration::from_millis(1000)).await;

    // init can task
    unsafe { cortex_m::peripheral::NVIC::unmask(embassy_rp::interrupt::PIO2_IRQ_0) };
    embassy_rp::interrupt::PIO2_IRQ_0.set_priority(Priority::P1);
    let pio_num = 2;
    let mut can = can2040_rs::Can2040::new(pio_num);
    can.setup();
    can.set_callback(Some(can_callback));
    let can_ptr = &mut can as *mut _;
    CAN_INSTANCE.store(can_ptr, Ordering::Release);
    let gpio_rx = 10; // goes to transceiver rx, do not flip
    let gpio_tx = 11; // goes to transceiver tx, do not flip
    let sys_clock = embassy_rp::clocks::clk_sys_freq();
    can.start(sys_clock, 500_000, gpio_rx, gpio_tx);
    unwrap!(spawner.spawn(can_task()));
}
