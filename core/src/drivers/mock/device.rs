//! A simulated pair of earbuds speaking the mock protocol. Swift can use it in
//! place of a Bluetooth channel: write the controller's outbound frames to
//! `receive`, and feed what it returns back into the controller.

use std::sync::{Arc, Mutex, MutexGuard};

use super::codec::{self, frame_type};
use super::{BAND_COUNT, MAX_GAIN_DB, MIN_GAIN_DB, preset_curve};
use crate::model::{AncMode, BatteryReading, BatteryStatus, FeatureToggle};
use super::MockPreset;

#[derive(uniffi::Object)]
pub struct MockEarbuds {
    sim: Mutex<SimState>,
}

struct SimState {
    battery: BatteryStatus,
    anc: AncMode,
    preset: MockPreset,
    gains: [i8; BAND_COUNT],
    /// On/off state per entry of `codec::TOGGLES`.
    toggles: [bool; 3],
    rx: Vec<u8>,
}

impl SimState {
    fn battery_frame(&self) -> Vec<u8> {
        codec::encode(frame_type::BATTERY, &codec::battery_payload(&self.battery))
    }

    fn anc_frame(&self) -> Vec<u8> {
        codec::encode(frame_type::ANC, &[codec::anc_to_byte(self.anc)])
    }

    fn toggle_frame(&self, toggle: FeatureToggle) -> Vec<u8> {
        let [id, _] = codec::toggle_payload(toggle, false);
        let on = self.toggles[id as usize];
        codec::encode(frame_type::TOGGLE, &codec::toggle_payload(toggle, on))
    }

    fn eq_frame(&self) -> Vec<u8> {
        codec::encode(frame_type::EQ, &codec::eq_payload(self.preset, &self.gains))
    }

    /// Apply one host command and return the device's reply bytes.
    /// Invalid commands are ignored silently, as real firmware tends to do.
    fn handle(&mut self, frame_type: u8, payload: &[u8]) -> Vec<u8> {
        match frame_type {
            frame_type::STATUS_REQUEST => {
                let toggles = codec::TOGGLES.iter().flat_map(|&t| self.toggle_frame(t));
                [self.battery_frame(), self.anc_frame(), self.eq_frame()]
                    .concat()
                    .into_iter()
                    .chain(toggles)
                    .collect()
            }
            frame_type::SET_ANC => match codec::decode_anc(payload) {
                Ok(mode) => {
                    self.anc = mode;
                    self.anc_frame()
                }
                Err(_) => Vec::new(),
            },
            frame_type::SET_TOGGLE => match codec::decode_toggle(payload) {
                Ok((toggle, on)) => {
                    self.toggles[codec::toggle_payload(toggle, on)[0] as usize] = on;
                    self.toggle_frame(toggle)
                }
                Err(_) => Vec::new(),
            },
            frame_type::SET_EQ => match codec::decode_eq(payload) {
                Ok((preset, gains)) => {
                    self.preset = preset;
                    // The device is authoritative for preset curves.
                    self.gains = preset_curve(preset)
                        .unwrap_or(gains.map(|g| g.clamp(MIN_GAIN_DB, MAX_GAIN_DB)));
                    self.eq_frame()
                }
                Err(_) => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}

impl MockEarbuds {
    fn sim(&self) -> MutexGuard<'_, SimState> {
        self.sim.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl MockEarbuds {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sim: Mutex::new(SimState {
                battery: BatteryStatus {
                    left: Some(BatteryReading { percent: 82, charging: false }),
                    right: Some(BatteryReading { percent: 78, charging: false }),
                    charging_case: Some(BatteryReading { percent: 64, charging: true }),
                },
                anc: AncMode::NoiseCancelling,
                preset: MockPreset::Flat,
                gains: preset_curve(MockPreset::Flat).expect("Flat has a curve"),
                toggles: [true, false, true],
                rx: Vec::new(),
            }),
        })
    }

    /// Bytes the host writes to the device. Returns the bytes the device sends
    /// back, which may hold zero or more frames.
    pub fn receive(&self, bytes: Vec<u8>) -> Vec<u8> {
        let mut sim = self.sim();
        sim.rx.extend_from_slice(&bytes);

        // Collect complete commands first so `rx` isn't borrowed while handling them.
        let mut commands = Vec::new();
        let mut offset = 0;
        while let Some((consumed, frame)) = codec::decode(&sim.rx[offset..]) {
            offset += consumed;
            if let Ok(frame) = frame {
                commands.push((frame.frame_type, frame.payload.to_vec()));
            }
        }
        sim.rx.drain(..offset);

        commands
            .into_iter()
            .flat_map(|(frame_type, payload)| sim.handle(frame_type, &payload))
            .collect()
    }

    /// Simulate time passing: each bud loses 1% battery. Returns the battery
    /// notification the device sends unprompted.
    pub fn tick(&self) -> Vec<u8> {
        let mut sim = self.sim();
        let battery = &mut sim.battery;
        for bud in [&mut battery.left, &mut battery.right].into_iter().flatten() {
            bud.percent = bud.percent.saturating_sub(1);
        }
        sim.battery_frame()
    }
}
