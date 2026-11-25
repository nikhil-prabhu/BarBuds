//! A simulated Fast Pair device for the app's Debug menu (see
//! [`crate::DeviceSimulator`]) and tests: earbuds being worn, with all three
//! noise-control modes settable. The noise-control request the driver sends
//! on connect is answered with the device information a real device sends
//! unasked (model ID, firmware, battery), then the noise-control state.
//! Unlike Pixel Buds Pro, it accepts unsigned noise-control changes, as some
//! devices may. Ring messages go to the Find simulator, as on a real device
//! whose control channel is also its Fast Pair channel.

use super::{AncInfo, anc_bit, code, group};
use std::sync::Arc;

use crate::fast_pair::{FindSimulator, decode, encode};
use crate::simulator::Simulator;

/// Device actions (ringing), handled by the Find simulator.
const DEVICE_ACTION: u8 = 0x04;

pub(crate) struct FastPairSim {
    rx: Vec<u8>,
    left: u8,
    right: u8,
    case: u8,
    anc: AncInfo,
    /// The device information has been sent.
    introduced: bool,
    /// Answers ring requests and acknowledgements of ring updates.
    find: Arc<FindSimulator>,
}

impl Default for FastPairSim {
    fn default() -> Self {
        let modes = anc_bit::OFF | anc_bit::NOISE_CANCELLING | anc_bit::TRANSPARENCY;
        Self {
            rx: Vec::new(),
            left: 90,
            right: 85,
            case: 60,
            anc: AncInfo {
                version: 1,
                supported: modes,
                settable: modes,
                current: anc_bit::NOISE_CANCELLING,
            },
            introduced: false,
            find: FindSimulator::new(),
        }
    }
}

impl FastPairSim {
    fn battery(&self) -> Vec<u8> {
        encode(
            group::DEVICE_INFO,
            code::BATTERY,
            &[self.left, self.right, self.case],
        )
    }

    fn handle(&mut self, group: u8, code: u8, data: &[u8]) -> Vec<u8> {
        match (group, code) {
            (group::HEARABLE_CONTROL, code::GET_ANC) => {
                let mut out = Vec::new();
                if !self.introduced {
                    self.introduced = true;
                    // A made-up model ID.
                    out.extend(encode(
                        group::DEVICE_INFO,
                        code::MODEL_ID,
                        &[0x00, 0x00, 0x01],
                    ));
                    out.extend(encode(group::DEVICE_INFO, code::FIRMWARE, b"1.0.0"));
                    out.extend(self.battery());
                }
                out.extend(encode(
                    group::HEARABLE_CONTROL,
                    code::NOTIFY_ANC,
                    &self.anc.bytes(),
                ));
                out
            }
            (group::HEARABLE_CONTROL, code::SET_ANC) => {
                let wanted = AncInfo::parse(data).map(|info| info.current);
                match wanted {
                    // Exactly one settable mode.
                    Ok(bit) if bit.count_ones() == 1 && self.anc.settable & bit != 0 => {
                        self.anc.current = bit;
                        let mut out = encode(group::ACKNOWLEDGEMENT, code::ACK, &[group, code]);
                        out.extend(encode(
                            group::HEARABLE_CONTROL,
                            code::NOTIFY_ANC,
                            &self.anc.bytes(),
                        ));
                        out
                    }
                    // NAK: reason 1 (not supported), then what it refuses.
                    _ => encode(group::ACKNOWLEDGEMENT, code::NAK, &[0x01, group, code]),
                }
            }
            (DEVICE_ACTION, _) | (group::ACKNOWLEDGEMENT, _) => {
                self.find.receive(encode(group, code, data))
            }
            _ => Vec::new(), // Real devices ignore what they don't know.
        }
    }
}

impl Simulator for FastPairSim {
    fn receive(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.rx.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((consumed, group, code, data)) = decode(&self.rx) {
            let data = data.to_vec();
            self.rx.drain(..consumed);
            out.extend(self.handle(group, code, &data));
        }
        out
    }

    fn tick(&mut self) -> Vec<u8> {
        self.left = self.left.saturating_sub(1);
        self.right = self.right.saturating_sub(1);
        let mut out = self.battery();
        out.extend(self.find.tick());
        out
    }
}
