#![allow(clippy::large_stack_frames, reason = "static embassy task futures live in task pool")]

use alloc::boxed::Box;

use embassy_usb_host::class::kbd::{KbdEvent, KbdHandler};
use embassy_usb_host::handler::HandlerEvent;
use embassy_usb_host::{BusRoute, BusState};
use esp_hal::usb::otg::embassy_usb_host as otg_host;
use log::{info, warn};
use static_cell::StaticCell;

use crate::report;

const CONFIG_DESCRIPTOR_BUF_LEN: usize = 256;

/// Drives the USB host: waits for the keyboard to attach, enumerates it, and
/// pumps HID boot-protocol reports until it goes away — then starts over.
#[embassy_executor::task]
pub async fn usb_host(driver: otg_host::Driver<'static>, bus_state: &'static BusState) -> ! {
    let (mut bus, handle) = embassy_usb_host::bus(driver, bus_state);

    static CONFIG_BUF: StaticCell<[u8; CONFIG_DESCRIPTOR_BUF_LEN]> = StaticCell::new();
    let config_buf = CONFIG_BUF.init([0u8; CONFIG_DESCRIPTOR_BUF_LEN]);

    loop {
        let speed = bus.wait_for_connection().await;

        let enum_info = match handle.enumerate(BusRoute::Direct(speed), config_buf).await {
            Ok((info, _len)) => info,
            Err(e) => {
                warn!("Enumeration failed: {}", e);
                continue;
            }
        };

        match Box::pin(KbdHandler::try_register(&handle, &enum_info)).await {
            Ok(mut kbd) => {
                info!("Keyboard registered, streaming reports");
                loop {
                    match kbd.wait_for_event().await {
                        Ok(HandlerEvent::HandlerEvent(KbdEvent::KeyStatusUpdate(update))) => {
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

        handle.free_address(enum_info.device_address);
    }
}
