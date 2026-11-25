//! Wire format for the mock protocol. Used by both the host-side driver and
//! the simulated device.

use super::BAND_COUNT;
use super::MockPreset;
use crate::error::BarBudsError;
use crate::model::{AncMode, BatteryReading, BatteryStatus, FeatureToggle};

pub(crate) const MAGIC: u8 = 0xBB;
const HEADER_LEN: usize = 3; // magic, type, len
const TRAILER_LEN: usize = 1; // checksum

pub(crate) mod frame_type {
    // Device -> host notifications.
    pub const BATTERY: u8 = 0x01;
    pub const ANC: u8 = 0x02;
    pub const EQ: u8 = 0x03;
    pub const TOGGLE: u8 = 0x04;
    // Host -> device commands.
    pub const STATUS_REQUEST: u8 = 0x80;
    pub const SET_ANC: u8 = 0x81;
    pub const SET_EQ: u8 = 0x82;
    pub const SET_TOGGLE: u8 = 0x83;
}

const BATTERY_ABSENT: u8 = 0x7F;
const CHARGING_BIT: u8 = 0x80;
const PRESET_CUSTOM: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawFrame<'a> {
    pub frame_type: u8,
    pub payload: &'a [u8],
}

/// XOR of the type byte, length byte and every payload byte.
pub(crate) fn checksum(frame_type: u8, payload: &[u8]) -> u8 {
    payload
        .iter()
        .fold(frame_type ^ payload.len() as u8, |acc, b| acc ^ b)
}

pub(crate) fn encode(frame_type: u8, payload: &[u8]) -> Vec<u8> {
    let len = u8::try_from(payload.len()).expect("mock payloads are under 256 bytes");
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len() + TRAILER_LEN);
    frame.extend_from_slice(&[MAGIC, frame_type, len]);
    frame.extend_from_slice(payload);
    frame.push(checksum(frame_type, payload));
    frame
}

/// Split one frame off the front of `buf`. `None` means more bytes are needed.
/// On error, the returned length is how many bytes to skip to resynchronise.
pub(crate) fn decode(buf: &[u8]) -> Option<(usize, Result<RawFrame<'_>, BarBudsError>)> {
    let first = *buf.first()?;
    if first != MAGIC {
        let skip = buf.iter().position(|&b| b == MAGIC).unwrap_or(buf.len());
        let err = BarBudsError::malformed(format!("skipped {skip} byte(s) before sync byte"));
        return Some((skip, Err(err)));
    }
    if buf.len() < HEADER_LEN {
        return None;
    }
    let frame_type = buf[1];
    let len = buf[2] as usize;
    let total = HEADER_LEN + len + TRAILER_LEN;
    if buf.len() < total {
        return None;
    }
    let payload = &buf[HEADER_LEN..HEADER_LEN + len];
    let actual = buf[total - 1];
    let expected = checksum(frame_type, payload);
    if actual != expected {
        // The length byte itself may be corrupt, so only drop the sync byte
        // and rescan from there rather than trusting `total`.
        return Some((
            1,
            Err(BarBudsError::ChecksumMismatch {
                expected: expected.into(),
                actual: actual.into(),
            }),
        ));
    }
    Some((
        total,
        Ok(RawFrame {
            frame_type,
            payload,
        }),
    ))
}

fn expect_len(payload: &[u8], len: usize, what: &str) -> Result<(), BarBudsError> {
    if payload.len() == len {
        Ok(())
    } else {
        Err(BarBudsError::malformed(format!(
            "{what} payload is {} byte(s), expected {len}",
            payload.len()
        )))
    }
}

// --- ANC ---

pub(crate) fn anc_to_byte(mode: AncMode) -> u8 {
    match mode {
        AncMode::Off => 0,
        AncMode::NoiseCancelling => 1,
        AncMode::Transparency => 2,
    }
}

pub(crate) fn decode_anc(payload: &[u8]) -> Result<AncMode, BarBudsError> {
    expect_len(payload, 1, "ANC")?;
    match payload[0] {
        0 => Ok(AncMode::Off),
        1 => Ok(AncMode::NoiseCancelling),
        2 => Ok(AncMode::Transparency),
        other => Err(BarBudsError::malformed(format!("unknown ANC mode {other}"))),
    }
}

// --- Battery: one byte each for left, right, case. Bit 7 = charging,
// bits 0-6 = percent, 0x7F = not reporting. ---

fn battery_to_byte(reading: Option<BatteryReading>) -> u8 {
    match reading {
        None => BATTERY_ABSENT,
        Some(r) => r.percent.min(100) | if r.charging { CHARGING_BIT } else { 0 },
    }
}

fn byte_to_battery(byte: u8) -> Result<Option<BatteryReading>, BarBudsError> {
    let percent = byte & !CHARGING_BIT;
    match percent {
        BATTERY_ABSENT => Ok(None),
        0..=100 => Ok(Some(BatteryReading {
            percent,
            charging: byte & CHARGING_BIT != 0,
        })),
        _ => Err(BarBudsError::malformed(format!(
            "battery {percent}% out of range"
        ))),
    }
}

pub(crate) fn battery_payload(status: &BatteryStatus) -> [u8; 3] {
    [
        battery_to_byte(status.left),
        battery_to_byte(status.right),
        battery_to_byte(status.charging_case),
    ]
}

pub(crate) fn decode_battery(payload: &[u8]) -> Result<BatteryStatus, BarBudsError> {
    expect_len(payload, 3, "battery")?;
    Ok(BatteryStatus {
        left: byte_to_battery(payload[0])?,
        right: byte_to_battery(payload[1])?,
        charging_case: byte_to_battery(payload[2])?,
    })
}

// --- EQ: [preset, band count, gain_db as i8 per band] ---

fn preset_to_byte(preset: MockPreset) -> u8 {
    match preset {
        MockPreset::Flat => 0,
        MockPreset::BassBoost => 1,
        MockPreset::Vocal => 2,
        MockPreset::TrebleBoost => 3,
        MockPreset::Custom => PRESET_CUSTOM,
    }
}

fn byte_to_preset(byte: u8) -> Result<MockPreset, BarBudsError> {
    match byte {
        0 => Ok(MockPreset::Flat),
        1 => Ok(MockPreset::BassBoost),
        2 => Ok(MockPreset::Vocal),
        3 => Ok(MockPreset::TrebleBoost),
        PRESET_CUSTOM => Ok(MockPreset::Custom),
        other => Err(BarBudsError::malformed(format!(
            "unknown EQ preset {other}"
        ))),
    }
}

pub(crate) fn eq_payload(preset: MockPreset, gains: &[i8; BAND_COUNT]) -> Vec<u8> {
    let mut payload = vec![preset_to_byte(preset), BAND_COUNT as u8];
    payload.extend(gains.iter().map(|&g| g as u8));
    payload
}

pub(crate) fn decode_eq(payload: &[u8]) -> Result<(MockPreset, [i8; BAND_COUNT]), BarBudsError> {
    expect_len(payload, 2 + BAND_COUNT, "EQ")?;
    let preset = byte_to_preset(payload[0])?;
    if payload[1] as usize != BAND_COUNT {
        return Err(BarBudsError::malformed(format!(
            "EQ band count {} (expected {BAND_COUNT})",
            payload[1]
        )));
    }
    let mut gains = [0i8; BAND_COUNT];
    for (gain, &byte) in gains.iter_mut().zip(&payload[2..]) {
        *gain = byte as i8;
    }
    Ok((preset, gains))
}

// --- FeatureToggle: [toggle id, 0 = off / 1 = on] ---

pub(crate) const TOGGLES: [FeatureToggle; 3] = [
    FeatureToggle::InEarDetection,
    FeatureToggle::AdaptiveSound,
    FeatureToggle::VolumeEq,
];

pub(crate) fn toggle_payload(toggle: FeatureToggle, on: bool) -> [u8; 2] {
    let id = TOGGLES
        .iter()
        .position(|&t| t == toggle)
        .expect("all toggles have ids");
    [id as u8, u8::from(on)]
}

pub(crate) fn decode_toggle(payload: &[u8]) -> Result<(FeatureToggle, bool), BarBudsError> {
    expect_len(payload, 2, "toggle")?;
    let toggle = *TOGGLES
        .get(payload[0] as usize)
        .ok_or_else(|| BarBudsError::malformed(format!("unknown toggle {}", payload[0])))?;
    match payload[1] {
        0 | 1 => Ok((toggle, payload[1] == 1)),
        other => Err(BarBudsError::malformed(format!("toggle value {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_anc_command() {
        // BB 81 01 02 | checksum = 0x81 ^ 0x01 ^ 0x02 = 0x82
        assert_eq!(
            encode(frame_type::SET_ANC, &[2]),
            [0xBB, 0x81, 0x01, 0x02, 0x82]
        );
    }

    #[test]
    fn decodes_battery_fixture() {
        // left 82%, right 78%, case 64% charging
        let fixture = [0xBB, 0x01, 0x03, 0x52, 0x4E, 0xC0, 0xDE];
        let (consumed, frame) = decode(&fixture).unwrap();
        assert_eq!(consumed, fixture.len());
        let frame = frame.unwrap();
        assert_eq!(frame.frame_type, frame_type::BATTERY);
        let battery = decode_battery(frame.payload).unwrap();
        assert_eq!(
            battery.left,
            Some(BatteryReading {
                percent: 82,
                charging: false
            })
        );
        assert_eq!(
            battery.right,
            Some(BatteryReading {
                percent: 78,
                charging: false
            })
        );
        assert_eq!(
            battery.charging_case,
            Some(BatteryReading {
                percent: 64,
                charging: true
            })
        );
    }

    #[test]
    fn absent_battery_component() {
        let payload = battery_payload(&BatteryStatus {
            left: None,
            ..Default::default()
        });
        assert_eq!(payload[0], BATTERY_ABSENT);
        assert_eq!(decode_battery(&payload).unwrap().left, None);
    }

    #[test]
    fn incomplete_frame_needs_more_bytes() {
        let frame = encode(frame_type::ANC, &[1]);
        for cut in 0..frame.len() {
            assert!(decode(&frame[..cut]).is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn skips_garbage_before_sync_byte() {
        let mut buf = vec![0x00, 0x11, 0x22];
        buf.extend(encode(frame_type::ANC, &[1]));
        let (consumed, result) = decode(&buf).unwrap();
        assert_eq!(consumed, 3);
        assert!(matches!(result, Err(BarBudsError::MalformedFrame { .. })));
    }

    #[test]
    fn bad_checksum_skips_only_sync_byte() {
        let mut frame = encode(frame_type::ANC, &[1]);
        *frame.last_mut().unwrap() ^= 0xFF;
        let (consumed, result) = decode(&frame).unwrap();
        assert_eq!(consumed, 1);
        assert!(matches!(result, Err(BarBudsError::ChecksumMismatch { .. })));
    }

    #[test]
    fn eq_round_trip_with_negative_gains() {
        let gains = [-6, -1, 0, 3, 6];
        let payload = eq_payload(MockPreset::Custom, &gains);
        assert_eq!(decode_eq(&payload).unwrap(), (MockPreset::Custom, gains));
    }
}
