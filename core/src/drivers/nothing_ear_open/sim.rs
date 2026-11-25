//! A simulated Nothing Ear (open) for the app's Debug menu (see
//! [`crate::DeviceSimulator`]) and tests. It answers the requests the driver
//! sends: gets are answered with a reply (direction `0x40`), sets with an
//! acknowledgement (`0x70`), and `tick` sends a battery notification.
//! Starting values match the tester's earbuds: 100% / 100%, case 65%,
//! Balanced EQ, Low Lag Mode off; Dual Connection on and a flat Custom curve
//! are guesses.

use super::{
    CONTROL_HAS_CRC, CRC_LEN, CUSTOM_CODE, HEADER_LEN, HOST_CONTROL, MAX_PAYLOAD, PRESETS, SOF,
    command, component, custom_eq_gains, custom_eq_payload, direction, encode_frame,
};
use crate::simulator::Simulator;

pub(crate) struct NothingEarOpenSim {
    rx: Vec<u8>,
    left: u8,
    right: u8,
    case: u8,
    /// Wire code of the current EQ preset.
    eq: u8,
    /// The Custom curve: Bass, Mid, Treble.
    custom: [f32; 3],
    low_latency: bool,
    dual_connection: bool,
}

impl Default for NothingEarOpenSim {
    fn default() -> Self {
        Self {
            rx: Vec::new(),
            left: 100,
            right: 100,
            case: 65,
            eq: 0,
            custom: [0.0; 3],
            low_latency: false,
            dual_connection: true,
        }
    }
}

impl NothingEarOpenSim {
    fn frame(direction: u8, command: u8, sequence: u8, payload: &[u8]) -> Vec<u8> {
        encode_frame(
            HOST_CONTROL,
            u16::from_le_bytes([command, direction]),
            sequence,
            payload,
        )
    }

    fn battery_payload(&self) -> Vec<u8> {
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

    fn handle(&mut self, direction: u8, command: u8, sequence: u8, payload: &[u8]) -> Vec<u8> {
        let reply = |payload: &[u8]| Self::frame(direction::REPLY, command, sequence, payload);
        let ack = || Self::frame(direction::ACK, command, sequence, &[]);
        match (direction, command) {
            (direction::GET, command::BATTERY) => reply(&self.battery_payload()),
            (direction::GET, command::FIRMWARE) => reply(b"1.0.0.0"),
            (direction::GET, command::EQ_GET) => reply(&[self.eq, 0]),
            (direction::GET, command::LATENCY_GET) => {
                reply(&[if self.low_latency { 1 } else { 2 }])
            }
            (direction::GET, command::DEVICE_INFO) => {
                reply(b"2,4,SIMLEFT01\n3,4,SIMRIGHT01\n4,4,SIMCASE01\n")
            }
            (direction::GET, command::DUAL_GET) => reply(&[u8::from(self.dual_connection)]),
            (direction::GET, command::CUSTOM_EQ_GET) => reply(&custom_eq_payload(self.custom)),
            (direction::SET, command::DUAL_SET) => match payload.first() {
                Some(&value @ (0 | 1)) => {
                    self.dual_connection = value == 1;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::CUSTOM_EQ_SET) => match custom_eq_gains(payload) {
                Ok(gains) if payload.len() == 53 => {
                    self.custom = gains;
                    ack()
                }
                _ => Vec::new(),
            },
            (direction::SET, command::EQ_SET) => match payload.first() {
                Some(&code)
                    if code == CUSTOM_CODE || PRESETS.iter().any(|&(_, _, c)| c == code) =>
                {
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

impl Simulator for NothingEarOpenSim {
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
