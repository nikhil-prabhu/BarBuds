//! Frames captured from real Pixel Buds Pro (firmware 5.11, both buds in an
//! open case) over the Maestro RFCOMM channel on macOS, 2026-11-23.
//!
//! The earbuds' `GetSoftwareInfo` replies are left out: they carry an
//! unidentified 64-bit value that may be a per-device identifier.

use super::rpc::{self, Method, RpcPacket, packet_type};
use crate::controller::BarBudsController;
use crate::model::{BatteryReading, DeviceModel, EqPreset, EqSettings};

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

/// Host -> earbuds: `ReadSetting(CURRENT_USER_EQ)` on channel 21, as BarBuds sent it.
const TX_READ_EQ: &str =
    "7e 00 4b 03 10 15 1d ea 71 de 7d 5e 25 51 ae d0 ae 2a 02 20 10 47 ee ad cf 7e";

/// One read chunk holding two frames:
/// 1. `SubscribeRuntimeInfo` stream: case 46% not charging, left and right
///    100% charging, both buds in the case.
/// 2. `ReadSetting(CURRENT_ANCR_STATE)` reply: state 0 (unknown, buds in case).
const RX_RUNTIME_INFO_AND_ANC: &str = "7e 00 a5 03 2a 25 10 cd 89 de d0 a0 34 18 00 32 12 0a 04 08 2e 10 01 \
    12 04 08 64 10 02 1a 04 08 64 10 02 3a 06 08 01 10 01 18 00 08 07 10 15 1d ea 71 de 7d 5e 25 \
    90 82 1e e6 61 71 da 50 7e 7e 00 a5 03 2a 04 22 02 68 00 08 01 10 15 1d ea 71 de 7d 5e 25 \
    51 ae d0 ae 65 1b 08 14 7e";

/// `ReadSetting(CURRENT_USER_EQ)` reply: -1, 0, +4, +2, 0 dB.
const RX_EQ: &str = "7e 00 a5 03 2a 1e 22 1c 82 01 19 0d 00 00 80 bf 15 00 00 00 00 1d 00 00 \
    80 40 25 00 00 00 40 2d 00 00 00 00 08 01 10 15 1d ea 71 de 7d 5e 25 51 ae d0 ae 9f 54 32 a9 7e";

/// `SERVER_ERROR` NOT_FOUND for the `GetSoftwareInfo` probe on channel 18 (the case).
const RX_PROBE_NOT_FOUND: &str = "7e 00 a3 03 08 05 10 12 1d ea 71 de 7d 5e 25 44 fa 99 71 30 05 38 ff ff ff ff 0f d1 69 51 26 7e";

/// Stands in for the earbuds' (omitted) successful probe reply on channel 21.
fn probe_reply(channel: u32) -> Vec<u8> {
    rpc::encode_packet(&RpcPacket {
        packet_type: packet_type::RESPONSE,
        channel_id: channel,
        service_id: rpc::MAESTRO_SERVICE,
        method_id: Method::GetSoftwareInfo.id(),
        payload: vec![],
        status: 0,
        call_id: u32::MAX,
    })
}

#[test]
fn captured_handshake() {
    let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
    controller.start();
    controller.feed(probe_reply(21)).unwrap();
    controller.take_outbound();

    controller.feed(hex(RX_RUNTIME_INFO_AND_ANC)).unwrap();
    // Reading ANC finished, so the driver asks for the EQ next, byte for byte
    // as the earbuds accepted it.
    assert_eq!(controller.take_outbound(), hex(TX_READ_EQ));

    controller.feed(hex(RX_EQ)).unwrap();
    // Then the Volume EQ setting (not captured: it was added later).
    assert!(!controller.take_outbound().is_empty());
    controller.feed(hex(RX_PROBE_NOT_FOUND)).unwrap();
    assert!(controller.take_outbound().is_empty());

    let state = controller.state();
    let charging = |percent| {
        Some(BatteryReading {
            percent,
            charging: true,
        })
    };
    assert_eq!(state.battery.left, charging(100));
    assert_eq!(state.battery.right, charging(100));
    assert_eq!(
        state.battery.charging_case,
        Some(BatteryReading {
            percent: 46,
            charging: false
        })
    );
    assert_eq!(state.anc, None);
    assert_eq!(
        state.eq,
        Some(EqSettings {
            preset: EqPreset::Custom,
            gains: vec![-1.0, 0.0, 4.0, 2.0, 0.0]
        })
    );
}
