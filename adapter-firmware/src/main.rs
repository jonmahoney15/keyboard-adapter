#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use alloc::boxed::Box;

use embassy_executor::Spawner;
use embassy_usb_host::class::kbd::{KbdEvent, KbdHandler};
use embassy_usb_host::handler::HandlerEvent;
use embassy_usb_host::{BusRoute, BusState};
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
// Aliased: esp-hal's integration module is itself named `embassy_usb_host`, and
// importing it unqualified would shadow the crate of the same name.
use esp_hal::usb::otg::{Usb, embassy_usb_host as otg_host};
use log::{info, warn};
use static_cell::StaticCell;

mod ble;
mod report;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

extern crate alloc;

/// Scratch space for the configuration descriptor fetched during enumeration.
/// A Moonlander's full config descriptor is well under this; 256 bytes leaves
/// room for composite keyboards that expose extra HID/RGB interfaces.
const CONFIG_DESCRIPTOR_BUF_LEN: usize = 256;

esp_bootloader_esp_idf::esp_app_desc!();

/// Feeds `esp-println`'s `timestamp` feature, if it is ever enabled.
#[unsafe(no_mangle)]
pub extern "Rust" fn _esp_println_timestamp() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis()
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger(log::LevelFilter::Info);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // Take over the OTG full-speed PHY (GPIO19 = D-, GPIO20 = D+) and bring it
    // up in host mode, so the keyboard downstream sees us as its host.
    let usb = Usb::new_fs(peripherals.USB_FS, peripherals.GPIO20, peripherals.GPIO19);
    let host_driver = otg_host::Driver::new(usb);

    // `BusState` is borrowed for `'d` by both halves of the bus, so it has to
    // outlive them — hence the `StaticCell` rather than a stack local.
    static BUS_STATE: StaticCell<BusState> = StaticCell::new();
    let bus_state = BUS_STATE.init(BusState::new());

    spawner.spawn(usb_host(host_driver, bus_state).unwrap());
    spawner.spawn(ble::ble_keyboard(peripherals.BT).unwrap());

    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}

/// Drives the USB host: waits for the keyboard to attach, enumerates it, and
/// pumps HID boot-protocol reports until it goes away — then starts over.
#[allow(
    clippy::large_stack_frames,
    reason = "this measures the size of the task's future, not a stack frame. An \
    embassy task future lives in the statically allocated task pool, so it cannot \
    overflow a stack; the pool is sized at compile time or the spawn fails."
)]
#[embassy_executor::task]
async fn usb_host(driver: otg_host::Driver<'static>, bus_state: &'static BusState) -> ! {
    let (mut bus, handle) = embassy_usb_host::bus(driver, bus_state);

    // Hoisted out of the loop and into the task's static allocation rather than
    // being re-materialized in the future on every reconnect.
    static CONFIG_BUF: StaticCell<[u8; CONFIG_DESCRIPTOR_BUF_LEN]> = StaticCell::new();
    let config_buf = CONFIG_BUF.init([0u8; CONFIG_DESCRIPTOR_BUF_LEN]);

    loop {
        info!("USB host waiting for a device on the root port");
        // Resolves only once a device has attached and the bus reset that
        // `wait_for_connection` drives has completed, so the device is sitting
        // in the default (address 0) state that `enumerate` requires.
        let speed = bus.wait_for_connection().await;
        info!("Device attached at {:?}", speed);

        let enum_info = match handle.enumerate(BusRoute::Direct(speed), config_buf).await {
            Ok((info, _len)) => info,
            Err(e) => {
                warn!("Enumeration failed: {}", e);
                continue;
            }
        };
        info!(
            "Enumerated vid={:04x} pid={:04x} at address {}",
            enum_info.device_desc.vendor_id,
            enum_info.device_desc.product_id,
            enum_info.device_address
        );

        // Boxed: `try_register` holds a 512-byte descriptor buffer across an
        // await, and inlining that into this task's future is what pushes the
        // task pool allocation over 1KB. The heap is already initialized in
        // `main`, and this runs once per connect, so the allocation is cheap.
        match Box::pin(KbdHandler::try_register(&handle, &enum_info)).await {
            Ok(mut kbd) => {
                info!("Keyboard registered, streaming reports");
                loop {
                    match kbd.wait_for_event().await {
                        Ok(HandlerEvent::HandlerEvent(KbdEvent::KeyStatusUpdate(update))) => {
                            // Non-blocking: the BLE task may have no central
                            // attached, and this loop must keep servicing the
                            // keyboard's interrupt IN endpoint regardless.
                            if !report::send(report::to_bytes(&update)) {
                                warn!("Report queue full, dropping report");
                            }
                        }
                        Ok(HandlerEvent::NoChange) => {}
                        Ok(HandlerEvent::HandlerDisconnected) => {
                            info!("Keyboard disconnected");
                            break;
                        }
                        Err(e) => {
                            warn!("Interrupt IN failed, dropping device: {:?}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => warn!("Device is not a boot-protocol keyboard: {:?}", e),
        }

        // Whether registration failed or the device went away, the address has
        // to go back to the pool or we leak one per reconnect.
        handle.free_address(enum_info.device_address);
    }
}
