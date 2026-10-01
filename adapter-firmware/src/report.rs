//! The boundary between the USB host side and the BLE side.
//!
//! The USB task produces 8-byte HID boot-protocol reports; the BLE task
//! consumes them and notifies them to the connected central. Keeping the two
//! sides behind a channel means neither has to know the other's state: the USB
//! task keeps draining the keyboard's interrupt IN endpoint whether or not a
//! phone is currently connected.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb_host::class::kbd::KeyStatusUpdate;

/// A HID boot-protocol keyboard input report, in the wire format that both the
/// USB boot protocol and the HID-over-GATT report map below use:
/// `[modifiers, reserved, keycode x6]`.
pub type BootReport = [u8; 8];

/// Depth of the USB → BLE queue.
///
/// Sized for burst absorption, not buffering: a human rolling over a few keys
/// can produce a short run of reports faster than one BLE connection interval,
/// and those should all land. Anything beyond that is a sign the link is gone
/// rather than slow, and is better dropped than queued — see [`send`].
const REPORT_QUEUE_DEPTH: usize = 8;

/// `CriticalSectionRawMutex` rather than `NoopRawMutex`: the two ends live in
/// different tasks, and on a multi-core part esp-rtos may run them on different
/// cores, so the channel cannot assume single-threaded access.
static REPORTS: Channel<CriticalSectionRawMutex, BootReport, REPORT_QUEUE_DEPTH> = Channel::new();

/// Flattens a `KeyStatusUpdate` back into the 8 bytes it was parsed from.
pub fn to_bytes(report: &KeyStatusUpdate) -> BootReport {
    let mut out = [0u8; 8];
    out[0] = report.modifiers;
    out[1] = report.reserved;
    for (slot, key) in out[2..].iter_mut().zip(report.keypress.iter()) {
        *slot = key.map_or(0, |k| k.get());
    }
    out
}

/// Queues a report for the BLE task, dropping it if the queue is full.
///
/// Deliberately non-blocking and non-async. The caller is the loop servicing
/// the keyboard's interrupt IN endpoint, and stalling that loop on a BLE link
/// that may have no central attached at all would wedge the USB side. A dropped
/// report is recoverable — the next one carries the full current key state, so
/// the host resynchronizes rather than being left with a stuck key.
///
/// Returns `false` if the report was dropped.
#[must_use]
pub fn send(report: BootReport) -> bool {
    REPORTS.try_send(report).is_ok()
}

/// Waits for the next report from the USB side.
pub async fn receive() -> BootReport {
    REPORTS.receive().await
}
