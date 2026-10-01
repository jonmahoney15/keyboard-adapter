#![allow(clippy::large_stack_frames)]

use bt_hci::controller::ExternalController;
use embassy_futures::select::{Either, select};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use log::{info, warn};
use trouble_host::prelude::*;

use crate::report;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

const HCI_COMMAND_SLOTS: usize = 4;

const DEVICE_NAME: &str = "Mahoney Keyboard";

/// HID boot-protocol keyboard report descriptor.
///
/// Describes exactly the 8-byte layout the USB boot protocol already gives us
/// (`[modifiers, reserved, keycode x6]`), so reports pass through untranslated:
/// 8 modifier bits, a padding byte, 5 LED output bits padded to a byte, and 6
/// key array bytes. Taken from the HID spec's boot-keyboard example (Appendix B).
#[rustfmt::skip]
const REPORT_MAP: [u8; 63] = [
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x06,       // Usage (Keyboard)
    0xA1, 0x01,       // Collection (Application)
    0x05, 0x07,       //   Usage Page (Key Codes)
    0x19, 0xE0,       //   Usage Minimum (224, LeftControl)
    0x29, 0xE7,       //   Usage Maximum (231, RightGUI)
    0x15, 0x00,       //   Logical Minimum (0)
    0x25, 0x01,       //   Logical Maximum (1)
    0x75, 0x01,       //   Report Size (1)
    0x95, 0x08,       //   Report Count (8)
    0x81, 0x02,       //   Input (Data, Variable, Absolute) -- modifier byte
    0x95, 0x01,       //   Report Count (1)
    0x75, 0x08,       //   Report Size (8)
    0x81, 0x01,       //   Input (Constant) -- reserved byte
    0x95, 0x05,       //   Report Count (5)
    0x75, 0x01,       //   Report Size (1)
    0x05, 0x08,       //   Usage Page (LEDs)
    0x19, 0x01,       //   Usage Minimum (1, NumLock)
    0x29, 0x05,       //   Usage Maximum (5, Kana)
    0x91, 0x02,       //   Output (Data, Variable, Absolute) -- LED report
    0x95, 0x01,       //   Report Count (1)
    0x75, 0x03,       //   Report Size (3)
    0x91, 0x01,       //   Output (Constant) -- LED report padding
    0x95, 0x06,       //   Report Count (6)
    0x75, 0x08,       //   Report Size (8)
    0x15, 0x00,       //   Logical Minimum (0)
    0x25, 0x65,       //   Logical Maximum (101)
    0x05, 0x07,       //   Usage Page (Key Codes)
    0x19, 0x00,       //   Usage Minimum (0)
    0x29, 0x65,       //   Usage Maximum (101)
    0x81, 0x00,       //   Input (Data, Array) -- key array
    0xC0,             // End Collection
];

const HID_INFORMATION: [u8; 4] = [0x11, 0x01, 0x00, 0x03];
const INPUT_REPORT_REFERENCE: [u8; 2] = [0x00, 0x01];
const PROTOCOL_MODE_REPORT: u8 = 0x01;

#[gatt_service(uuid = service::HUMAN_INTERFACE_DEVICE)]
struct HidService {
    #[characteristic(uuid = characteristic::REPORT_MAP, read, value = REPORT_MAP)]
    report_map: [u8; 63],

    #[characteristic(uuid = characteristic::HID_INFORMATION, read, value = HID_INFORMATION)]
    hid_information: [u8; 4],

    #[characteristic(
        uuid = characteristic::REPORT,
        read, notify,
        permissions (encrypted),
        value = [0u8; 8],
    )]
    #[descriptor(
        uuid = descriptors::REPORT_REFERENCE,
        read,
        value = INPUT_REPORT_REFERENCE,
    )]
    input_report: report::BootReport,

    #[characteristic(
        uuid = characteristic::PROTOCOL_MODE,
        read, write_without_response,
        value = PROTOCOL_MODE_REPORT,
    )]
    protocol_mode: u8,

    #[characteristic(uuid = characteristic::HID_CONTROL_POINT, write_without_response, value = 0u8)]
    hid_control_point: u8,
}

#[gatt_server(connections_max = CONNECTIONS_MAX)]
struct Server {
    hid: HidService,
}

#[embassy_executor::task]
pub async fn ble_keyboard(bt: BT<'static>) -> ! {
    let transport = BleConnector::new(bt, Default::default()).unwrap();
    let controller = ExternalController::<_, HCI_COMMAND_SLOTS>::new(transport);

    let mut resources: HostResources<_, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_io_capabilities(IoCapabilities::NoInputNoOutput)
        .build();

    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: DEVICE_NAME,
        appearance: &appearance::human_interface_device::KEYBOARD,
    }))
    .unwrap();

    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();

    match select(runner.run(), advertise_and_serve(&mut peripheral, &server)).await {
        Either::First(Err(e)) => panic!("BLE host runner stopped: {:?}", e),
        Either::First(Ok(())) => panic!("BLE host runner stopped unexpectedly"),
        Either::Second(never) => never,
    }
}

async fn advertise_and_serve<C>(
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
) -> !
where
    C: Controller
        + for<'t> bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetAdvData>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetAdvParams>
        + for<'t> bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetAdvEnable>
        + for<'t> bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanResponseData>,
{
    let mut adv_data = [0u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids16(&[service::HUMAN_INTERFACE_DEVICE.to_le_bytes()]),
            AdStructure::Unknown {
                ty: 0x19, // Appearance
                data: &appearance::human_interface_device::KEYBOARD.to_le_bytes(),
            },
        ],
        &mut adv_data,
    )
    .unwrap();

    let mut scan_data = [0u8; 31];
    let scan_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(DEVICE_NAME.as_bytes())],
        &mut scan_data,
    )
    .unwrap();

    loop {
        info!("BLE advertising as {:?}", DEVICE_NAME);
        let advertiser = match peripheral
            .advertise(
                &Default::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_data[..adv_len],
                    scan_data: &scan_data[..scan_len],
                },
            )
            .await
        {
            Ok(advertiser) => advertiser,
            Err(e) => {
                warn!("Failed to start advertising: {:?}", e);
                continue;
            }
        };

        let conn = match advertiser.accept().await {
            Ok(conn) => match conn.with_attribute_server(server) {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("Failed to attach attribute server: {:?}", e);
                    continue;
                }
            },
            Err(e) => {
                warn!("Failed to accept connection: {:?}", e);
                continue;
            }
        };
        info!("Central connected");

        serve(&conn, server).await;
        info!("Central disconnected, restarting advertising");
    }
}

async fn serve(conn: &GattConnection<'_, '_, DefaultPacketPool>, server: &Server<'_>) {
    loop {
        match select(conn.next(), report::receive()).await {
            Either::First(GattConnectionEvent::Disconnected { reason }) => {
                info!("Disconnected: {:?}", reason);
                return;
            }
            Either::First(GattConnectionEvent::Gatt { event }) => {
                match event.accept() {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("Failed to handle GATT event: {:?}", e),
                }
            }
            Either::First(_) => {}
            Either::Second(input_report) => {
                if let Err(e) = server
                    .hid
                    .input_report
                    .notify(conn, &input_report, true)
                    .await
                {
                    warn!("Failed to notify input report: {:?}", e);
                }
            }
        }
    }
}
