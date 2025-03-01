#![no_std]
#![no_main]

mod ble_protocol;
mod ble_server;
mod can_manager;
mod channels;
mod isotp_ble_bridge;
mod isotp_handler;
mod led;

use bt_hci::controller::ExternalController;
use cyw43::bluetooth::BtDriver;
use cyw43_pio::PioSpi;
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, UART1};
use embassy_rp::pio::{self, Pio};
use embassy_rp::uart::{self};
use embassy_time::{Duration, Timer};
use fixed::FixedU32;
use static_cell::StaticCell;
use {defmt_serial as _, panic_probe as _};

// Program metadata for `picotool info`.
#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 4] = [
    embassy_rp::binary_info::rp_program_name!(c"BLE_TO_ISOTP"),
    embassy_rp::binary_info::rp_program_description!(c"BLE to ISOTP bridge"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

// interrupt handlers
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    PIO2_IRQ_0 => can_manager::CanInterruptHandler;
});

// cyw43 task
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

// ble task
#[embassy_executor::task]
async fn ble_task(bt_device: BtDriver<'static>) {
    let controller: ExternalController<BtDriver<'static>, 10> = ExternalController::new(bt_device);
    ble_server::run::<_, 128>(controller).await;
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
        FixedU32::from_bits(0x400), // do not use RM2_CLOCK_DIVIDER or DEFAULT_CLOCK_DIVIDER?
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

    // sleep to allow cyw43 to settle
    Timer::after(Duration::from_millis(250)).await;

    // init led task
    static CONTROL: StaticCell<cyw43::Control<'static>> = StaticCell::new();
    let control = CONTROL.init(control);
    unwrap!(spawner.spawn(led::led_task(control)));

    // sleep to allow cyw43 to settle
    Timer::after(Duration::from_millis(250)).await;

    // init ble peripheral
    unwrap!(spawner.spawn(ble_task(bt_device)));

    // sleep to allow cyw43 to settle
    Timer::after(Duration::from_millis(250)).await;

    // init can bus

    can_manager::init_can();

    // sleep to allow can to settle
    Timer::after(Duration::from_millis(250)).await;

    unwrap!(spawner.spawn(can_manager::can_tx_channel_task()));
    unwrap!(spawner.spawn(can_manager::can_rx_channel_task()));
    unwrap!(spawner.spawn(can_manager::can_stats_task()));
    unwrap!(spawner.spawn(can_manager::can_reset_task()));

    // init ble isotp bridge
    unwrap!(spawner.spawn(isotp_ble_bridge::isotp_ble_bridge_ble_rx_task()));
    unwrap!(spawner.spawn(isotp_ble_bridge::isotp_ble_bridge_can_rx_task()));

    // tasks will run in background
}
