//! A simulated Nothing or CMF device for the app's Debug menu (see
//! [`crate::DeviceSimulator`]) and tests: any model in [`super::models`],
//! the Ear (open) by default. It answers the requests the driver sends: gets
//! with a reply (direction `0x40`), sets with an acknowledgement (`0x70`),
//! and `tick` sends a battery notification. It only answers the requests the
//! model supports, as real earbuds ignore the rest.
//!
//! The Ear (open) starts as the tester's did: 100% / 100%, case 65%,
//! Balanced EQ, Low Lag Mode off; Dual Connection on and a flat Custom curve
//! are guesses, and gestures start at BudsLink's factory defaults. Other
//! models start with their first preset, Noise Cancelling at its default
//! strength, and each gesture on its first action.

use super::models::{self, NothingModel};
use super::{
    CONTROL_HAS_CRC, CRC_LEN, CUSTOM_CODE, HEADER_LEN, HOST_CONTROL, MAX_PAYLOAD, SOF, command,
    component, custom_eq_gains, custom_eq_payload, direction, encode_frame, gesture_entry,
    gesture_settings,
};
use crate::model::{BudSide, ControlAction, ControlGesture, ControlSetting};
use crate::simulator::Simulator;

pub(crate) struct NothingSim {
    model: &'static NothingModel,
    rx: Vec<u8>,
    left: u8,
    right: u8,
    case: u8,
    /// Wire code of the current EQ preset.
    eq: u8,
    /// The Custom curve: Bass, Mid, Treble.
    custom: [f32; 3],
    /// Wire code of the noise-control mode.
    anc: u8,
    low_latency: bool,
    dual_connection: bool,
    in_ear_detection: bool,
    gestures: Vec<ControlSetting>,
}

impl Default for NothingSim {
    fn default() -> Self {
        let ear_open = models::model(0xB174).expect("the Ear (open) is listed");
        let mut sim = Self::new(ear_open);
        sim.gestures = {
            use BudSide::*;
            use ControlAction::*;
            use ControlGesture::*;
            [
                (Left, Double, SkipForward),
                (Right, Double, SkipForward),
                (Left, Triple, SkipBack),
                (Right, Triple, SkipBack),
                (Left, Hold, VolumeDown),
                (Right, Hold, VolumeUp),
                (Left, DoubleHold, NoAction),
                (Right, DoubleHold, NoAction),
            ]
            .into_iter()
            .map(|(side, gesture, action)| ControlSetting {
                side,
                gesture,
                action,
            })
            .collect()
        };
        sim
    }
}

impl NothingSim {
    pub(crate) fn new(model: &'static NothingModel) -> Self {
        Self {
            model,
            rx: Vec::new(),
            left: 100,
            right: 100,
            case: 65,
            eq: model.presets.first().map_or(0, |&(_, _, code)| code),
            custom: [0.0; 3],
            anc: model.anc.map_or(0, |anc| anc.cancelling[0]),
            low_latency: false,
            dual_connection: true,
            in_ear_detection: true,
            gestures: model
                .gestures
                .iter()
                .map(|slot| ControlSetting {
                    side: slot.side,
                    gesture: slot.gesture,
                    action: slot.actions[0],
                })
                .collect(),
        }
    }

    fn frame(direction: u8, command: u8, sequence: u8, payload: &[u8]) -> Vec<u8> {
        encode_frame(
            HOST_CONTROL,
            u16::from_le_bytes([command, direction]),
            sequence,
            payload,
        )
    }

    fn battery_payload(&self) -> Vec<u8> {
        if self.model.single_battery {
            return vec![1, component::BOTH, self.left];
        }
        vec![
            3,
            component::LEFT,
            self.left,
            component::RIGHT,
            self.right,
            component::CASE,
            self.case,
        ]
    }

    fn valid_eq(&self, code: u8) -> bool {
        (self.model.custom_sliders && code == CUSTOM_CODE)
            || self.model.presets.iter().any(|&(_, _, c)| c == code)
    }

    fn valid_anc(&self, code: u8) -> bool {
        self.model.anc.is_some_and(|anc| {
            code == anc.off || Some(code) == anc.transparency || anc.cancelling.contains(&code)
        })
    }

    fn handle(&mut self, direction: u8, command: u8, sequence: u8, payload: &[u8]) -> Vec<u8> {
        let reply = |payload: &[u8]| Self::frame(direction::REPLY, command, sequence, payload);
        let ack = || Self::frame(direction::ACK, command, sequence, &[]);
        let model = self.model;
        let eq_set = if model.listening_mode {
            command::LISTENING_SET
        } else {
            command::EQ_SET
        };
        match (direction, command) {
            (direction::GET, command::BATTERY) => reply(&self.battery_payload()),
            (direction::GET, command::FIRMWARE) => reply(b"1.0.0.0"),
            (direction::GET, command::MODEL_ID) => reply(&model.id.to_le_bytes()),
            (direction::GET, command::EQ_GET) => reply(&[self.eq, 0]),
            (direction::GET, command::LATENCY_GET) => {
                reply(&[if self.low_latency { 1 } else { 2 }])
            }
            (direction::GET, command::DEVICE_INFO) => {
                reply(b"2,4,SIMLEFT01\n3,4,SIMRIGHT01\n4,4,SIMCASE01\n")
            }
            (direction::GET, command::DUAL_GET) if model.dual_connection => {
                reply(&[u8::from(self.dual_connection)])
            }
            (direction::GET, command::IN_EAR_GET) if model.in_ear_detection => {
                reply(&[1, 1, u8::from(self.in_ear_detection)])
            }
            (direction::GET, command::ANC_GET) if model.anc.is_some() => reply(&[1, self.anc, 0]),
            (direction::GET, command::CUSTOM_EQ_GET) if model.custom_sliders => {
                reply(&custom_eq_payload(self.custom))
            }
            (direction::GET, command::GESTURES_GET) if !model.gestures.is_empty() => {
                let mut payload = vec![self.gestures.len() as u8];
                payload.extend(
                    self.gestures
                        .iter()
                        .filter_map(|&g| gesture_entry(g))
                        .flatten(),
                );
                reply(&payload)
            }
            (direction::SET, command::GESTURES_SET) if !model.gestures.is_empty() => {
                match gesture_settings(payload) {
                    Ok(changes) if !changes.is_empty() => {
                        for change in changes {
                            if let Some(existing) = self
                                .gestures
                                .iter_mut()
                                .find(|g| g.side == change.side && g.gesture == change.gesture)
                            {
                                existing.action = change.action;
                            }
                        }
                        ack()
                    }
                    _ => Vec::new(),
                }
            }
            (direction::SET, command::DUAL_SET) if model.dual_connection => match payload.first() {
                Some(&value @ (0 | 1)) => {
                    self.dual_connection = value == 1;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::IN_EAR_SET) if model.in_ear_detection => match payload {
                [1, 1, value @ (0 | 1), ..] => {
                    self.in_ear_detection = *value == 1;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::ANC_SET) => match payload {
                [1, code, ..] if self.valid_anc(*code) => {
                    self.anc = *code;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::CUSTOM_EQ_SET) if model.custom_sliders => {
                match custom_eq_gains(payload) {
                    Ok(gains) if payload.len() == 53 => {
                        self.custom = gains;
                        ack()
                    }
                    _ => Vec::new(),
                }
            }
            (direction::SET, set) if set == eq_set => match payload.first() {
                Some(&code) if self.valid_eq(code) => {
                    self.eq = code;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::LATENCY_SET) => match payload.first() {
                Some(&value @ (1 | 2)) => {
                    self.low_latency = value == 1;
                    ack()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(), // Real earbuds ignore what they don't know.
        }
    }
}

impl Simulator for NothingSim {
    fn receive(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.rx.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            // Resync on the start-of-frame byte.
            match self.rx.iter().position(|&b| b == SOF) {
                Some(0) => {}
                Some(skip) => {
                    self.rx.drain(..skip);
                }
                None => {
                    self.rx.clear();
                    break;
                }
            }
            if self.rx.len() < HEADER_LEN {
                break;
            }
            let control = u16::from_le_bytes([self.rx[1], self.rx[2]]);
            let len = u16::from_le_bytes([self.rx[5], self.rx[6]]) as usize;
            if len > MAX_PAYLOAD {
                self.rx.remove(0);
                continue;
            }
            let crc_len = if control & CONTROL_HAS_CRC != 0 {
                CRC_LEN
            } else {
                0
            };
            let total = HEADER_LEN + len + crc_len;
            if self.rx.len() < total {
                break;
            }
            let frame: Vec<u8> = self.rx.drain(..total).collect();
            let (command, direction, sequence) = (frame[3], frame[4], frame[7]);
            out.extend(self.handle(
                direction,
                command,
                sequence,
                &frame[HEADER_LEN..HEADER_LEN + len],
            ));
        }
        out
    }

    fn tick(&mut self) -> Vec<u8> {
        self.left = self.left.saturating_sub(1);
        self.right = self.right.saturating_sub(1);
        Self::frame(
            direction::NOTIFY_ALT,
            command::BATTERY_NOTIFY,
            0,
            &self.battery_payload(),
        )
    }
}
