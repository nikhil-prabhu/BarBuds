//! Google Pixel Buds A-Series.
//!
//! The control channel is RFCOMM, advertised under [`SERVICE_UUID`]. Every
//! message, in both directions:
//! ```text
//! +----------+------+-------------+------------------+
//! | endpoint | type | len (u16 BE)| payload[len]     |
//! +----------+------+-------------+------------------+
//! ```
//! There is no sync byte or checksum (RFCOMM already checks integrity).
//! Endpoint `0x87` carries control messages with protobuf payloads; `0x02`
//! carries firmware updates and `0x05` firmware debug logs, both ignored.
//!
//! Control messages (types on endpoint `0x87`):
//! - `0x04` host asks for device info; the earbuds answer with `0x05`
//!   (hardware info), `0x02` (firmware versions) and `0x06` (runtime info:
//!   battery, same fields as Maestro's `RuntimeInfo`), and send `0x06` again
//!   whenever the battery changes.
//! - `0x08` read setting: `{3: id}`.
//! - `0x09` write setting: `{3: {id: value}}`.
//! - `0x0a` setting value, in the same shape as a write. Sent in reply to a
//!   read, and again once a written value takes effect.
//! - `0x01` acknowledgement: `{1: type of the message acknowledged}`.
//!
//! The bass slider (setting 14) stores levels -1…4 as 0…5. It maps to a
//! one-band EQ in [`EqUnit::Level`]s with no presets (a custom curve only).
//!
//! Reverse-engineered from Bluetooth HCI captures of the Pixel Buds app on a
//! Pixel phone changing each setting (2026-11-23).

pub(crate) mod sim;

use prost::Message;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::drivers::google::{self, FirmwareVersion, HardwareInfo, RuntimeInfo, SoftwareInfo};
use crate::error::BarBudsError;
use crate::model::{
    AncMode, DeviceCapabilities, EqCapabilities, EqSettings, EqUnit, FeatureToggle, FormFactor,
};

/// RFCOMM service UUID of the control channel.
pub(crate) const SERVICE_UUID: &str = "3a046f6d-24d2-7655-6534-0d7ecb759709";

const HEADER_LEN: usize = 4;
/// Longest payload seen is a 236-byte firmware chunk; anything far larger
/// means the stream is out of sync.
const MAX_PAYLOAD: usize = 2048;

mod endpoint {
    pub const FIRMWARE_UPDATE: u8 = 0x02;
    pub const LOG: u8 = 0x05;
    pub const CONTROL: u8 = 0x87;
}

mod message {
    pub const SOFTWARE_INFO: u8 = 0x02;
    pub const INFO_REQUEST: u8 = 0x04;
    pub const HARDWARE_INFO: u8 = 0x05;
    pub const RUNTIME_INFO: u8 = 0x06;
    pub const READ_SETTING: u8 = 0x08;
    pub const WRITE_SETTING: u8 = 0x09;
    pub const SETTING_VALUE: u8 = 0x0a;
}

/// Setting IDs, matched to the app's controls in the captures.
mod setting {
    pub const IN_EAR_DETECTION: u32 = 1;
    pub const ADAPTIVE_SOUND: u32 = 5;
    pub const VOLUME_EQ: u32 = 13;
    pub const BASS_LEVEL: u32 = 14;
}

/// The app's bass slider runs from -1 to 4; the device stores `level + 1`.
const MIN_BASS: i64 = -1;
const MAX_BASS: i64 = 4;
const BASS_OFFSET: i64 = 1;

fn bass_eq(level: i64) -> EqSettings {
    EqSettings {
        preset: None,
        gains: vec![level as f32],
    }
}

const TOGGLES: [(FeatureToggle, u32); 3] = [
    (FeatureToggle::AdaptiveSound, setting::ADAPTIVE_SOUND),
    (FeatureToggle::VolumeEq, setting::VOLUME_EQ),
    (FeatureToggle::InEarDetection, setting::IN_EAR_DETECTION),
];

/// Payload of a read request: `{3: id}`.
#[derive(Clone, PartialEq, Message)]
struct ReadSetting {
    #[prost(uint32, tag = "3")]
    id: u32,
}

/// Payload of a write or a reported value: `{3: <setting>}`, where the inner
/// message has a single varint field whose number is the setting ID.
#[derive(Clone, PartialEq, Message)]
struct SettingMessage {
    #[prost(bytes = "vec", tag = "3")]
    setting: Vec<u8>,
}

fn encode(endpoint: u8, message_type: u8, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(payload.len()).expect("control payloads are small");
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&[endpoint, message_type]);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn read_setting(id: u32) -> Vec<u8> {
    encode(
        endpoint::CONTROL,
        message::READ_SETTING,
        &ReadSetting { id }.encode_to_vec(),
    )
}

fn push_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn take_varint(bytes: &mut &[u8]) -> Result<u64, BarBudsError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes
            .split_first()
            .ok_or_else(|| BarBudsError::malformed("truncated varint"))?;
        *bytes = rest;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(BarBudsError::malformed("varint too long"))
}

/// The `(id, value)` in a setting write or report.
fn decode_setting(payload: &[u8]) -> Result<(u32, u64), BarBudsError> {
    let message = SettingMessage::decode(payload)
        .map_err(|err| BarBudsError::malformed(format!("bad setting message: {err}")))?;
    let mut inner = message.setting.as_slice();
    let key = take_varint(&mut inner)?;
    if key & 0x7 != 0 {
        return Err(BarBudsError::malformed(format!(
            "setting {} is not a varint",
            key >> 3
        )));
    }
    let id =
        u32::try_from(key >> 3).map_err(|_| BarBudsError::malformed("setting ID overflows"))?;
    Ok((id, take_varint(&mut inner)?))
}

fn encode_setting(id: u32, value: u64) -> Vec<u8> {
    let mut setting = Vec::new();
    push_varint(u64::from(id) << 3, &mut setting);
    push_varint(value, &mut setting);
    SettingMessage { setting }.encode_to_vec()
}

/// The A-Series report a six-digit build such as "300581", which the Pixel
/// Buds app shows as "3.581.0" (confirmed on hardware): the first digit is the
/// major version and the rest the minor. Anything else is shown as reported.
fn display_version(version: &FirmwareVersion) -> Option<String> {
    let build = version.build.as_str();
    if build.is_empty() {
        return None;
    }
    let (major, minor) = build.split_at(1);
    match (build.len(), major.parse::<u32>(), minor.parse::<u32>()) {
        (6, Ok(major), Ok(minor)) => Some(format!("{major}.{minor}.0")),
        _ => Some(build.to_string()),
    }
}

/// Writes a setting, then reads it back, as the Pixel Buds app does.
fn write_setting(id: u32, value: u64) -> Vec<u8> {
    let mut frames = encode(
        endpoint::CONTROL,
        message::WRITE_SETTING,
        &encode_setting(id, value),
    );
    frames.extend(read_setting(id));
    frames
}

#[derive(Default)]
pub(crate) struct PixelBudsASeriesDriver;

impl PixelBudsASeriesDriver {
    fn handle(&self, message_type: u8, payload: &[u8]) -> Result<Option<Event>, BarBudsError> {
        match message_type {
            message::SOFTWARE_INFO => {
                let info = SoftwareInfo::decode(payload)
                    .map_err(|err| BarBudsError::malformed(format!("bad software info: {err}")))?;
                // The case entry just repeats the buds' version (the Pixel
                // Buds app showed a different case version), so leave it out.
                let firmware = google::firmware(&info, display_version).map(|mut firmware| {
                    firmware.charging_case = None;
                    firmware
                });
                Ok(firmware.map(Event::Firmware))
            }
            message::HARDWARE_INFO => {
                let info = HardwareInfo::decode(payload)
                    .map_err(|err| BarBudsError::malformed(format!("bad hardware info: {err}")))?;
                Ok(google::serial_numbers(&info).map(Event::SerialNumbers))
            }
            message::RUNTIME_INFO => {
                let info = RuntimeInfo::decode(payload)
                    .map_err(|err| BarBudsError::malformed(format!("bad runtime info: {err}")))?;
                Ok(info
                    .battery_info
                    .map(|b| Event::Battery(google::battery_status(&b))))
            }
            message::SETTING_VALUE => {
                let (id, value) = decode_setting(payload)?;
                if id == setting::BASS_LEVEL {
                    let level = i64::try_from(value).unwrap_or(i64::MAX) - BASS_OFFSET;
                    if !(MIN_BASS..=MAX_BASS).contains(&level) {
                        return Err(BarBudsError::malformed(format!(
                            "bass level {value} out of range"
                        )));
                    }
                    return Ok(Some(Event::Eq(bass_eq(level))));
                }
                let Some(&(toggle, _)) = TOGGLES.iter().find(|(_, toggle_id)| *toggle_id == id)
                else {
                    return Ok(None); // A setting BarBuds doesn't show.
                };
                match value {
                    0 | 1 => Ok(Some(Event::Toggle(toggle, value == 1))),
                    other => Err(BarBudsError::malformed(format!(
                        "{toggle:?} reported value {other}"
                    ))),
                }
            }
            // Acks, device info and our own message types echoed back carry no state.
            _ => Ok(None),
        }
    }
}

impl EarbudDriver for PixelBudsASeriesDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            display_name: "Pixel Buds A-Series".to_string(),
            anc_modes: vec![],
            eq: Some(EqCapabilities {
                band_labels: vec!["Bass".to_string()],
                unit: EqUnit::Level,
                min_gain: MIN_BASS as f32,
                max_gain: MAX_BASS as f32,
                step: 1.0,
                presets: vec![],
            }),
            has_charging_case: true,
            form_factor: FormFactor::Stemless,
            toggles: TOGGLES.iter().map(|&(toggle, _)| toggle).collect(),
            can_ring: true,
        }
    }

    fn start(&mut self) -> Vec<u8> {
        let mut frames = encode(endpoint::CONTROL, message::INFO_REQUEST, &[]);
        for (_, id) in TOGGLES {
            frames.extend(read_setting(id));
        }
        frames.extend(read_setting(setting::BASS_LEVEL));
        frames
    }

    fn parse_frame(&mut self, buf: &[u8], _outbound: &mut Vec<u8>) -> Option<Decoded> {
        let &endpoint = buf.first()?;
        if ![endpoint::CONTROL, endpoint::FIRMWARE_UPDATE, endpoint::LOG].contains(&endpoint) {
            let err = BarBudsError::malformed(format!("unknown endpoint {endpoint:#04x}"));
            return Some(Decoded {
                consumed: 1,
                result: Err(err),
            });
        }
        if buf.len() < HEADER_LEN {
            return None;
        }
        let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        if len > MAX_PAYLOAD {
            let err = BarBudsError::malformed(format!("{len}-byte message"));
            return Some(Decoded {
                consumed: 1,
                result: Err(err),
            });
        }
        let total = HEADER_LEN + len;
        if buf.len() < total {
            return None;
        }
        let result = if endpoint == endpoint::CONTROL {
            self.handle(buf[1], &buf[HEADER_LEN..total])
        } else {
            Ok(None)
        };
        Some(Decoded {
            consumed: total,
            result,
        })
    }

    fn encode_anc(&mut self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        Err(BarBudsError::Unsupported {
            feature: format!("ANC mode {mode:?}"),
        })
    }

    fn encode_eq(&mut self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let [gain] = eq.gains[..] else {
            return Err(BarBudsError::invalid_argument(format!(
                "expected 1 EQ band (bass), got {}",
                eq.gains.len()
            )));
        };
        if !gain.is_finite() {
            return Err(BarBudsError::invalid_argument(
                "bass level is not a finite number",
            ));
        }
        let level = (gain.round() as i64).clamp(MIN_BASS, MAX_BASS);
        Ok(write_setting(
            setting::BASS_LEVEL,
            (level + BASS_OFFSET) as u64,
        ))
    }

    fn encode_toggle(&mut self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        let &(_, id) = TOGGLES.iter().find(|(t, _)| *t == toggle).ok_or_else(|| {
            BarBudsError::Unsupported {
                feature: format!("{toggle:?}"),
            }
        })?;
        Ok(write_setting(id, u64::from(on)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::BarBudsController;
    use crate::model::{BatteryReading, DeviceModel};

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    // Fixtures captured from real Pixel Buds A-Series talking to the Pixel
    // Buds app (Bluetooth HCI snoop log on a Pixel phone, 2026-11-23). The
    // hardware-info (serial numbers) and firmware-info messages are left out.

    /// Runtime info with both buds out of the case: left and right 100%, not
    /// charging; no case reading.
    const RX_RUNTIME_INFO: &str = "87 06 00 21 08 b2 d1 10 10 a6 c5 8b cb a0 34 18 1b 32 0c 12 04 08 64 \
        10 01 1a 04 08 64 10 01 3a 04 08 00 10 00";

    /// The replies to the app's reads of settings 8, 1, 3, 5, 7, 8, 10, 11,
    /// 12, 13 and 14, each followed by an ack of the read (`08 08`).
    const RX_SETTINGS: &str = "87 01 00 02 08 04 \
        87 0a 00 04 1a 02 40 01  87 01 00 02 08 08  87 01 00 02 08 07 \
        87 0a 00 04 1a 02 08 01  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 18 00  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 28 00  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 38 01  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 40 01  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 50 00  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 58 01  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 60 00  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 68 01  87 01 00 02 08 08 \
        87 0a 00 04 1a 02 70 03  87 01 00 02 08 08";

    /// The app turning Volume EQ off: write, then read back.
    const TX_VOLUME_EQ_OFF: &str = "87 09 00 04 1a 02 68 00 87 08 00 02 18 0d";
    /// The app turning Adaptive Sound on.
    const TX_ADAPTIVE_SOUND_ON: &str = "87 09 00 04 1a 02 28 01 87 08 00 02 18 05";
    /// The app turning in-ear detection off.
    const TX_IN_EAR_OFF: &str = "87 09 00 04 1a 02 08 00 87 08 00 02 18 01";

    /// The earbuds' answer to `TX_VOLUME_EQ_OFF`: ack of the write, the
    /// read reply, ack of the read, then the change notification.
    const RX_VOLUME_EQ_OFF: &str = "87 01 00 02 08 09 87 0a 00 04 1a 02 68 00 \
        87 01 00 02 08 08 87 0a 00 04 1a 02 68 00";

    fn connected() -> std::sync::Arc<BarBudsController> {
        let controller = BarBudsController::new(DeviceModel::PixelBudsASeries);
        controller.start();
        controller.feed(hex(RX_RUNTIME_INFO)).unwrap();
        controller.feed(hex(RX_SETTINGS)).unwrap();
        controller
    }

    #[test]
    fn start_requests_info_and_toggle_settings() {
        let mut driver = PixelBudsASeriesDriver;
        assert_eq!(
            driver.start(),
            hex(
                "87 04 00 00 87 08 00 02 18 05 87 08 00 02 18 0d 87 08 00 02 18 01 \
                 87 08 00 02 18 0e"
            )
        );
    }

    #[test]
    fn captured_initial_state() {
        let state = connected().state();
        let full = Some(BatteryReading {
            percent: 100,
            charging: false,
        });
        assert_eq!(state.battery.left, full);
        assert_eq!(state.battery.right, full);
        assert_eq!(state.battery.charging_case, None);
        assert_eq!(state.toggles.len(), 3, "other settings are ignored");
        assert!(state.toggles[&FeatureToggle::InEarDetection]);
        assert!(!state.toggles[&FeatureToggle::AdaptiveSound]);
        assert!(state.toggles[&FeatureToggle::VolumeEq]);
        assert_eq!(state.anc, None);
        // Setting 14 = 3: the app's bass slider at +2.
        assert_eq!(state.eq, Some(bass_eq(2)));
    }

    #[test]
    fn toggle_commands_match_the_app() {
        let controller = connected();
        for (toggle, on, expected) in [
            (FeatureToggle::VolumeEq, false, TX_VOLUME_EQ_OFF),
            (FeatureToggle::AdaptiveSound, true, TX_ADAPTIVE_SOUND_ON),
            (FeatureToggle::InEarDetection, false, TX_IN_EAR_OFF),
        ] {
            assert_eq!(
                controller.encode_toggle(toggle, on).unwrap(),
                hex(expected),
                "{toggle:?}"
            );
        }
    }

    #[test]
    fn toggle_applies_after_device_confirms() {
        let controller = connected();
        controller
            .encode_toggle(FeatureToggle::VolumeEq, false)
            .unwrap();
        assert!(controller.state().toggles[&FeatureToggle::VolumeEq]);
        controller.feed(hex(RX_VOLUME_EQ_OFF)).unwrap();
        assert!(!controller.state().toggles[&FeatureToggle::VolumeEq]);
    }

    #[test]
    fn handles_byte_at_a_time_delivery() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsASeries);
        for byte in hex(RX_RUNTIME_INFO).into_iter().chain(hex(RX_SETTINGS)) {
            controller.feed(vec![byte]).unwrap();
        }
        assert_eq!(controller.state().toggles.len(), 3);
        assert!(controller.state().battery.left.is_some());
    }

    #[test]
    fn ignores_logs_and_firmware_messages() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsASeries);
        let mut bytes = encode(endpoint::LOG, 0x01, b"debug text");
        bytes.extend(encode(endpoint::FIRMWARE_UPDATE, 0x0c, &[0; 8]));
        bytes.extend(hex(RX_VOLUME_EQ_OFF));
        controller.feed(bytes).unwrap();
        assert!(!controller.state().toggles[&FeatureToggle::VolumeEq]);
    }

    #[test]
    fn resyncs_after_garbage() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsASeries);
        let mut bytes = vec![0xFF, 0x00];
        bytes.extend(hex(RX_VOLUME_EQ_OFF));
        let err = controller.feed(bytes).unwrap_err();
        assert!(matches!(err, BarBudsError::MalformedFrame { .. }));
        assert!(!controller.state().toggles[&FeatureToggle::VolumeEq]);
    }

    // The bass slider moved to -1, +4, then back to +2 (second capture).
    const TX_BASS_MIN: &str = "87 09 00 04 1a 02 70 00 87 08 00 02 18 0e";
    const TX_BASS_MAX: &str = "87 09 00 04 1a 02 70 05 87 08 00 02 18 0e";
    const RX_BASS_MAX: &str = "87 01 00 02 08 09 87 0a 00 04 1a 02 70 05 \
        87 01 00 02 08 08 87 0a 00 04 1a 02 70 05";

    #[test]
    fn bass_slider_commands_match_the_app() {
        let controller = connected();
        let set = |level: f32| {
            controller.encode_eq(EqSettings {
                preset: None,
                gains: vec![level],
            })
        };
        assert_eq!(set(-1.0).unwrap(), hex(TX_BASS_MIN));
        assert_eq!(set(4.0).unwrap(), hex(TX_BASS_MAX));
        // Out-of-range and fractional slider values snap to valid levels.
        assert_eq!(set(9.0).unwrap(), hex(TX_BASS_MAX));
        assert_eq!(set(3.6).unwrap(), hex(TX_BASS_MAX));
    }

    #[test]
    fn bass_applies_after_device_confirms() {
        let controller = connected();
        controller.feed(hex(RX_BASS_MAX)).unwrap();
        assert_eq!(controller.state().eq, Some(bass_eq(4)));
    }

    #[test]
    fn bass_rejects_wrong_band_count() {
        let controller = connected();
        let eq = EqSettings {
            preset: None,
            gains: vec![0.0, 0.0],
        };
        assert!(matches!(
            controller.encode_eq(eq),
            Err(BarBudsError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn firmware_and_serial_numbers() {
        // Synthetic `87 02` and `87 05` messages in the captured layout; the
        // real ones carry identifying values, so they aren't fixtures.
        let version = |build: &str| {
            Some(FirmwareVersion {
                build: build.to_string(),
                version_string: String::new(),
            })
        };
        let software = SoftwareInfo {
            firmware: Some(google::FirmwareInfo {
                case: version("300581"),
                right: version("300581"),
                left: version("300581"),
            }),
        };
        let hardware = HardwareInfo {
            serial_numbers: Some(google::SerialNumbers {
                case: "SIMCASE01".to_string(),
                right: "SIMUR01".to_string(),
                left: "SIMUL01".to_string(),
            }),
        };
        let controller = BarBudsController::new(DeviceModel::PixelBudsASeries);
        let mut bytes = encode(
            endpoint::CONTROL,
            message::SOFTWARE_INFO,
            &software.encode_to_vec(),
        );
        bytes.extend(encode(
            endpoint::CONTROL,
            message::HARDWARE_INFO,
            &hardware.encode_to_vec(),
        ));
        controller.feed(bytes).unwrap();

        let info = controller.state().info;
        // What the Pixel Buds app showed for these builds.
        assert_eq!(info.firmware.left.as_deref(), Some("3.581.0"));
        assert_eq!(info.firmware.right.as_deref(), Some("3.581.0"));
        // Not reported: the case entry repeats the buds' version.
        assert_eq!(info.firmware.charging_case, None);
        assert_eq!(info.serial_numbers.left.as_deref(), Some("SIMUL01"));
        assert_eq!(
            info.serial_numbers.charging_case.as_deref(),
            Some("SIMCASE01")
        );
    }

    #[test]
    fn unexpected_build_formats_are_shown_as_is() {
        let version = |build: &str| FirmwareVersion {
            build: build.to_string(),
            version_string: String::new(),
        };
        assert_eq!(display_version(&version("1.2.3")).as_deref(), Some("1.2.3"));
        assert_eq!(display_version(&version("12345")).as_deref(), Some("12345"));
        assert_eq!(display_version(&version("")), None);
    }

    #[test]
    fn has_bass_only_eq_and_no_anc() {
        let controller = connected();
        assert!(matches!(
            controller.encode_anc(AncMode::Off),
            Err(BarBudsError::Unsupported { .. })
        ));
        let caps = controller.capabilities();
        assert!(caps.anc_modes.is_empty());
        let eq = caps.eq.unwrap();
        assert_eq!(eq.band_labels, ["Bass"]);
        assert_eq!(eq.unit, EqUnit::Level);
        assert!(eq.presets.is_empty());
        // Presets other than Custom are refused before reaching the driver.
        let flat = EqSettings {
            preset: Some("flat".to_string()),
            gains: vec![],
        };
        assert!(matches!(
            controller.encode_eq(flat),
            Err(BarBudsError::Unsupported { .. })
        ));
    }
}
