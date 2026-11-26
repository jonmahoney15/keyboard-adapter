#![allow(
    clippy::large_stack_frames,
    reason = "static embassy task futures live in task pool"
)]

use embedded_graphics::{image::Image, pixelcolor::BinaryColor, prelude::Point, prelude::*};
use esp_hal::{Async, i2c::master::I2c};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, prelude::*};

const DANCING_FERRIS_FRAMES: [&[u8]; 8] = [
    include_bytes!("../assets/ferris_00.bmp"),
    include_bytes!("../assets/ferris_01.bmp"),
    include_bytes!("../assets/ferris_02.bmp"),
    include_bytes!("../assets/ferris_03.bmp"),
    include_bytes!("../assets/ferris_04.bmp"),
    include_bytes!("../assets/ferris_05.bmp"),
    include_bytes!("../assets/ferris_06.bmp"),
    include_bytes!("../assets/ferris_07.bmp"),
];

const FRAME_COUNT: usize = DANCING_FERRIS_FRAMES.len();
const MIN_X: i32 = 16;
const MAX_X: i32 = 96;
const STEP: i32 = 16;

#[embassy_executor::task]
pub async fn ferris_dance(i2c_bus: I2c<'static, Async>) -> ! {
    let interface = I2CDisplayInterface::new(i2c_bus);

    let mut display = Ssd1306Async::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().await.unwrap();

    let bmps: [tinybmp::Bmp<'_, BinaryColor>; FRAME_COUNT] =
        core::array::from_fn(|i| tinybmp::Bmp::from_slice(DANCING_FERRIS_FRAMES[i]).unwrap());

    let mut x = MIN_X;
    let mut direction = STEP;

    loop {
        for bmp in &bmps {
            display.clear_buffer();
            Image::with_center(bmp, Point::new(x, 40))
                .draw(&mut display)
                .unwrap();
            display.flush().await.unwrap();
            embassy_time::Timer::after(embassy_time::Duration::from_millis(60)).await;
        }

        if x > MAX_X || x < MIN_X {
            direction = -direction;
        }
        x += direction;
    }
}
