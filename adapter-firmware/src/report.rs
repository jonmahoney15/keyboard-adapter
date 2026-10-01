use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb_host::class::kbd::KeyStatusUpdate;

pub type BootReport = [u8; 8];

const REPORT_QUEUE_DEPTH: usize = 8;

static REPORTS: Channel<CriticalSectionRawMutex, BootReport, REPORT_QUEUE_DEPTH> = Channel::new();

pub fn to_bytes(report: &KeyStatusUpdate) -> BootReport {
    let mut out = [0u8; 8];
    out[0] = report.modifiers;
    out[1] = report.reserved;
    for (slot, key) in out[2..].iter_mut().zip(report.keypress.iter()) {
        *slot = key.map_or(0, |k| k.get());
    }
    out
}

#[must_use]
pub fn send(report: BootReport) -> bool {
    REPORTS.try_send(report).is_ok()
}

pub async fn receive() -> BootReport {
    REPORTS.receive().await
}
