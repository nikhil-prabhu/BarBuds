//! A simulated pair of Pixel Buds A-Series for the app's Debug menu (see
//! [`crate::DeviceSimulator`]) and tests. It answers the control channel the
//! way the captured earbuds did:
//! - an info request (`87 04`) gets hardware info, firmware and battery;
//! - a read (`87 08`) gets the value (`87 0a`) and an acknowledgement (`87 01`);
//! - a write (`87 09`) gets an acknowledgement, then the new value again as a
//!   change notification.

use std::collections::HashMap;

use prost::Message;

use super::{
    HEADER_LEN, MAX_PAYLOAD, ReadSetting, decode_setting, encode, encode_setting, endpoint,
    message, setting,
};
use crate::drivers::google::{
    BatteryInfo, DeviceBatteryInfo, FirmwareInfo, FirmwareVersion, HardwareInfo, RuntimeInfo,
    SerialNumbers, SoftwareInfo, battery_state,
};
use crate::simulator::Simulator;

/// Acknowledges a host message: `{1: type of the message acknowledged}`.
const ACK: u8 = 0x01;

pub(crate) struct ASeriesSim {
    rx: Vec<u8>,
    /// Setting values by ID, as the earbuds store them.
    settings: HashMap<u32, u64>,
    left: i32,
    right: i32,
    case: i32,
}

impl Default for ASeriesSim {
    fn default() -> Self {
        Self {
            rx: Vec::new(),
            // The values captured from real earbuds: in-ear detection and
            // Volume EQ on, Adaptive Sound off, bass slider at +2 (stored as 3).
            settings: HashMap::from([
                (setting::IN_EAR_DETECTION, 1),
                (setting::ADAPTIVE_SOUND, 0),
                (setting::VOLUME_EQ, 1),
                (setting::BASS_LEVEL, 3),
            ]),
            left: 88,
            right: 86,
            case: 64,
        }
    }
}

fn control(message_type: u8, payload: &[u8]) -> Vec<u8> {
    encode(endpoint::CONTROL, message_type, payload)
}

fn ack(acknowledged: u8) -> Vec<u8> {
    control(ACK, &[0x08, acknowledged])
}

impl ASeriesSim {
    fn value(&self, id: u32) -> Option<Vec<u8>> {
        let value = *self.settings.get(&id)?;
        Some(control(message::SETTING_VALUE, &encode_setting(id, value)))
    }

    fn runtime_info(&self) -> Vec<u8> {
        let reading = |level: i32| {
            Some(DeviceBatteryInfo {
                level,
                state: battery_state::NOT_CHARGING,
            })
        };
        let info = RuntimeInfo {
            battery_info: Some(BatteryInfo {
                left: reading(self.left),
                right: reading(self.right),
                case: reading(self.case),
            }),
        };
        control(message::RUNTIME_INFO, &info.encode_to_vec())
    }

    fn handle(&mut self, message_type: u8, payload: &[u8]) -> Vec<u8> {
        match message_type {
            message::INFO_REQUEST => {
                let version = || {
                    Some(FirmwareVersion {
                        build: "300581".to_string(),
                        version_string: String::new(),
                    })
                };
                let software = SoftwareInfo {
                    firmware: Some(FirmwareInfo {
                        case: version(),
                        right: version(),
                        left: version(),
                    }),
                };
                let hardware = HardwareInfo {
                    serial_numbers: Some(SerialNumbers {
                        case: "SIMCASE01".to_string(),
                        right: "SIMUR01".to_string(),
                        left: "SIMUL01".to_string(),
                    }),
                };
                [
                    ack(message::INFO_REQUEST),
                    control(message::HARDWARE_INFO, &hardware.encode_to_vec()),
                    control(message::SOFTWARE_INFO, &software.encode_to_vec()),
                    self.runtime_info(),
                ]
                .concat()
            }
            message::READ_SETTING => {
                let mut out = ReadSetting::decode(payload)
                    .ok()
                    .and_then(|read| self.value(read.id))
                    .unwrap_or_default();
                out.extend(ack(message::READ_SETTING));
                out
            }
            message::WRITE_SETTING => {
                let mut out = ack(message::WRITE_SETTING);
                if let Ok((id, value)) = decode_setting(payload)
                    && self.settings.contains_key(&id)
                {
                    self.settings.insert(id, value);
                    out.extend(self.value(id).unwrap_or_default());
                }
                out
            }
            _ => Vec::new(),
        }
    }
}

impl Simulator for ASeriesSim {
    fn receive(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.rx.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.rx.len() < HEADER_LEN {
                break;
            }
            let len = u16::from_be_bytes([self.rx[2], self.rx[3]]) as usize;
            if len > MAX_PAYLOAD {
                self.rx.remove(0); // Out of sync: skip a byte, like the driver.
                continue;
            }
            if self.rx.len() < HEADER_LEN + len {
                break;
            }
            let frame: Vec<u8> = self.rx.drain(..HEADER_LEN + len).collect();
            if frame[0] == endpoint::CONTROL {
                out.extend(self.handle(frame[1], &frame[HEADER_LEN..]));
            }
        }
        out
    }

    fn tick(&mut self) -> Vec<u8> {
        self.left = (self.left - 1).max(0);
        self.right = (self.right - 1).max(0);
        self.runtime_info()
    }
}
