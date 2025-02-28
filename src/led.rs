use cyw43::Control;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

pub static LED_CHANNEL: Channel<ThreadModeRawMutex, LedCommand, 4> = Channel::new();

#[derive(Debug, Clone, Copy)]
pub enum LedCommand {
    Blink,
}

#[embassy_executor::task]
pub async fn led_task(control: &'static mut Control<'static>) {
    let receiver = LED_CHANNEL.receiver();

    loop {
        match receiver.receive().await {
            LedCommand::Blink => {
                control.gpio_set(0, true).await;
                Timer::after(Duration::from_millis(100)).await;
                control.gpio_set(0, false).await;
            }
        }
    }
}

// Helper function to send blink commands
pub async fn blink() {
    LED_CHANNEL.send(LedCommand::Blink).await;
}
