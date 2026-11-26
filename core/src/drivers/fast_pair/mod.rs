//! Any earbuds or headphones with Google Fast Pair, via the Fast Pair
//! message stream (the RFCOMM channel
//! [`FAST_PAIR_SERVICE_UUID`](crate::fast_pair::FAST_PAIR_SERVICE_UUID), framed as
//! described in [`crate::fast_pair`]). A fallback for devices without a
//! specific driver: it offers only what the standard covers.
//!
//! - **On connect** the device sends, unasked, its model ID, firmware
//!   revision and battery levels (device information, group `0x03`). Some
//!   devices skip the battery (a JBL Tune 770NC sent none in 15 minutes).
//! - **Battery** (`03 03`): one byte per component, left, right, case, or a
//!   single byte for one-battery devices such as headphones. Bit 7 means
//!   charging; `0x7F` means unknown. Until a three-part report arrives, the
//!   device is shown as headphones (one piece).
//! - **Firmware** (`03 09`): a version string.
//! - **Noise control** (hearable control, group `0x08`): get `11`, set `12`,
//!   notify `13`. A notification carries `[version, supported, settable,
//!   current]`, the last three as mode bits. A device may change which modes
//!   are settable while connected (Pixel Buds Pro report none while they
//!   charge in the case or aren't worn), so the state says whether the
//!   supported modes can be changed right now. A set sends `[version,
//!   supported, settable, wanted]`, but the spec requires it to carry a MAC
//!   made with the Fast Pair account key that the user's phone wrote into the
//!   device, which a Mac doesn't have; Pixel Buds Pro refuse it (NAK). So
//!   once a device reports a settable mode, BarBuds probes by setting the
//!   mode that's already active (a no-op if accepted), and on a refusal
//!   shows noise control read-only.
//! - **Ringing** uses the same channel: the app's `FindSession` shares it, and
//!   this driver ignores the ring messages (device action, group `0x04`).
//!
//! Sources: Google's Fast Pair Message Stream specification; BudsLink's
//! generic Fast Pair driver (GPL-3.0, <https://github.com/maniacx/BudsLink>;
//! facts only, no code) for the noise-control layout and mode bits. Captured
//! from Pixel Buds Pro (2026-11): the connect burst and a noise-control
//! notification.

pub(crate) mod sim;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::error::BarBudsError;
use crate::fast_pair::{decode, encode};
use crate::model::{
    AncAvailability, AncMode, BatteryReading, BatteryStatus, ComponentStrings, DeviceCapabilities,
    EqSettings, FormFactor,
};

pub(crate) mod group {
    pub const DEVICE_INFO: u8 = 0x03;
    pub const HEARABLE_CONTROL: u8 = 0x08;
    pub const ACKNOWLEDGEMENT: u8 = 0xFF;
}

pub(crate) mod code {
    pub const MODEL_ID: u8 = 0x01;
    pub const BATTERY: u8 = 0x03;
    pub const FIRMWARE: u8 = 0x09;
    pub const GET_ANC: u8 = 0x11;
    pub const SET_ANC: u8 = 0x12;
    pub const NOTIFY_ANC: u8 = 0x13;
    pub const ACK: u8 = 0x01;
    pub const NAK: u8 = 0x02;
}

/// Noise-control mode bits (BudsLink's). Adaptive (0x40) has no `AncMode`
/// yet, so it's never offered and shows as no mode.
pub(crate) mod anc_bit {
    pub const NOISE_CANCELLING: u8 = 0x08;
    pub const OFF: u8 = 0x20;
    pub const TRANSPARENCY: u8 = 0x80;
}

/// The modes BarBuds can show, in the popover's order, with their bits.
const MODES: [(AncMode, u8); 3] = [
    (AncMode::Off, anc_bit::OFF),
    (AncMode::NoiseCancelling, anc_bit::NOISE_CANCELLING),
    (AncMode::Transparency, anc_bit::TRANSPARENCY),
];

const UNKNOWN_LEVEL: u8 = 0x7F;
const CHARGING: u8 = 0x80;

/// The latest noise-control notification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AncInfo {
    pub version: u8,
    pub supported: u8,
    pub settable: u8,
    pub current: u8,
}

impl AncInfo {
    pub fn parse(data: &[u8]) -> Result<Self, BarBudsError> {
        match *data {
            [version, supported, settable, current, ..] => Ok(Self {
                version,
                supported,
                settable,
                current,
            }),
            _ => Err(BarBudsError::malformed("short noise-control payload")),
        }
    }

    pub fn bytes(self) -> [u8; 4] {
        [self.version, self.supported, self.settable, self.current]
    }

    /// The modes the device has, settable now or not.
    fn supported_modes(self) -> Vec<AncMode> {
        MODES
            .iter()
            .filter(|&&(_, bit)| self.supported & bit != 0)
            .map(|&(mode, _)| mode)
            .collect()
    }

    fn is_settable(self, bit: u8) -> bool {
        self.supported & self.settable & bit != 0
    }

    /// The current mode, `None` if it's one BarBuds can't show (Adaptive).
    fn current_mode(self) -> Option<AncMode> {
        MODES
            .iter()
            .find(|&&(_, bit)| self.current == bit)
            .map(|&(mode, _)| mode)
    }
}

fn reading(byte: u8) -> Option<BatteryReading> {
    let level = byte & !CHARGING;
    (level != UNKNOWN_LEVEL && level <= 100).then_some(BatteryReading {
        percent: level,
        charging: byte & CHARGING != 0,
    })
}

/// Battery levels; a single byte is a one-battery device, shown as `left`.
pub(crate) fn battery_status(data: &[u8]) -> Result<BatteryStatus, BarBudsError> {
    match *data {
        [single] => Ok(BatteryStatus {
            left: reading(single),
            right: None,
            charging_case: None,
        }),
        [left, right, case, ..] => Ok(BatteryStatus {
            left: reading(left),
            right: reading(right),
            charging_case: reading(case),
        }),
        _ => Err(BarBudsError::malformed(format!(
            "{}-byte battery payload",
            data.len()
        ))),
    }
}

#[derive(Default)]
pub(crate) struct FastPairDriver {
    /// The number of battery components last reported: 1 for headphones.
    battery_components: Option<usize>,
    /// The latest noise-control notification.
    anc: Option<AncInfo>,
    /// The device refused a mode change (it requires a MAC BarBuds can't make).
    anc_refused: bool,
    /// The no-op probe (see the module docs) has been sent.
    anc_probed: bool,
}

/// NAK reasons that mean a set will never be accepted: "not supported" and
/// "incorrect message authentication code". Others (busy, not allowed in
/// the current state, redundant) don't.
const REFUSED: [u8; 2] = [0x00, 0x03];

impl FastPairDriver {
    fn anc_availability(&self) -> AncAvailability {
        let settable = self
            .anc
            .is_some_and(|info| MODES.iter().any(|&(_, bit)| info.is_settable(bit)));
        if self.anc_refused {
            AncAvailability::OnDeviceOnly
        } else if settable {
            AncAvailability::Available
        } else {
            AncAvailability::NotNow
        }
    }

    fn handle(&mut self, group: u8, code: u8, data: &[u8]) -> Result<Option<Event>, BarBudsError> {
        match (group, code) {
            (group::DEVICE_INFO, code::BATTERY) => {
                let battery = battery_status(data)?;
                self.battery_components = Some(if data.len() == 1 { 1 } else { 3 });
                Ok(Some(Event::Battery(battery)))
            }
            (group::DEVICE_INFO, code::FIRMWARE) => {
                let version = String::from_utf8_lossy(data)
                    .trim_matches(char::from(0))
                    .trim()
                    .to_string();
                if version.is_empty() {
                    return Ok(None);
                }
                // One version for the whole product; shown as a single row.
                let version = Some(version);
                Ok(Some(Event::Firmware(ComponentStrings {
                    left: version.clone(),
                    right: version.clone(),
                    charging_case: version,
                })))
            }
            (group::HEARABLE_CONTROL, code::NOTIFY_ANC) => {
                let info = AncInfo::parse(data)?;
                self.anc = Some(info);
                Ok(Some(Event::AncState(
                    info.current_mode(),
                    self.anc_availability(),
                )))
            }
            (group::ACKNOWLEDGEMENT, code::NAK)
                if matches!(*data, [reason, group::HEARABLE_CONTROL, code::SET_ANC, ..]
                    if REFUSED.contains(&reason)) =>
            {
                // Most likely the missing MAC (see the module docs): stop
                // offering changes for this connection.
                self.anc_refused = true;
                Ok(Some(Event::AncAvailability(AncAvailability::OnDeviceOnly)))
            }
            // Model ID, address and session updates, ACKs (the new mode
            // arrives as a notification), ring messages (handled by the
            // app's FindSession on this same channel), and anything newer.
            _ => Ok(None),
        }
    }
}

impl EarbudDriver for FastPairDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        // Earbuds send a left/right/case battery report on connect (Pixel Buds
        // Pro do); a device that sends one level, or none at all (a JBL Tune
        // 770NC sent none), is treated as one piece. The Bluetooth device
        // class can't tell them apart: both report "Headset".
        let headphones = self.battery_components != Some(3);
        DeviceCapabilities {
            display_name: "Generic Fast Pair".to_string(),
            anc_modes: self.anc.map(AncInfo::supported_modes).unwrap_or_default(),
            eq: None,
            has_charging_case: !headphones,
            form_factor: if headphones {
                FormFactor::Headphones
            } else {
                FormFactor::Stemmed
            },
            toggles: vec![],
            // Over this same channel; see the module docs.
            can_ring: true,
            has_balance: false,
            anc_cycle_modes: vec![],
            spatial_audio_modes: vec![],
            controls: None,
        }
    }

    fn start(&mut self) -> Vec<u8> {
        *self = Self::default();
        // The device sends its information unasked; noise control is asked for.
        encode(group::HEARABLE_CONTROL, code::GET_ANC, &[])
    }

    fn parse_frame(&mut self, buf: &[u8], outbound: &mut Vec<u8>) -> Option<Decoded> {
        let (consumed, group, code, data) = decode(buf)?;
        if (group, code) == (0, 0) && consumed == 1 {
            return Some(Decoded {
                consumed,
                result: Err(BarBudsError::malformed("lost sync with the message stream")),
            });
        }
        let result = self.handle(group, code, data);
        // Find out whether changes are accepted before the user tries one.
        if (group, code) == (group::HEARABLE_CONTROL, code::NOTIFY_ANC)
            && !self.anc_probed
            && self.anc_availability() == AncAvailability::Available
            && let Some(info) = self.anc.filter(|info| info.is_settable(info.current))
        {
            self.anc_probed = true;
            outbound.extend(encode(
                group::HEARABLE_CONTROL,
                code::SET_ANC,
                &info.bytes(),
            ));
        }
        Some(Decoded { consumed, result })
    }

    fn encode_anc(&mut self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        let info = self.anc.ok_or_else(|| {
            BarBudsError::device("the earbuds haven't reported noise control yet")
        })?;
        let &(_, bit) = MODES
            .iter()
            .find(|&&(m, _)| m == mode)
            .expect("every AncMode has a bit");
        if !info.is_settable(bit) {
            return Err(BarBudsError::device(format!(
                "the earbuds can't switch to {mode:?} right now"
            )));
        }
        let wanted = AncInfo {
            current: bit,
            ..info
        };
        Ok(encode(
            group::HEARABLE_CONTROL,
            code::SET_ANC,
            &wanted.bytes(),
        ))
    }

    fn encode_eq(&mut self, _eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        Err(BarBudsError::Unsupported {
            feature: "EQ".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::BarBudsController;
    use crate::model::DeviceModel;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    /// Captured from Pixel Buds Pro on connect (2026-11), charging in the
    /// open case, after asking for the noise-control state. The BLE address
    /// message (`03 02`) is replaced with zeros and the session nonce
    /// (`03 0a`) with a made-up one.
    const PIXEL_BUDS_PRO_CONNECT: &str = "03 0a 00 08 01 02 03 04 05 06 07 08 \
        03 01 00 03 6e da f7 \
        03 02 00 06 00 00 00 00 00 00 \
        03 09 00 0a 52 65 76 69 73 69 6f 6e 20 35 \
        07 10 00 00 \
        03 03 00 03 e4 e4 5c 03 03 00 03 e4 e4 5c 03 03 00 03 e4 e4 5c \
        08 13 00 04 01 a8 00 20";

    #[test]
    fn decodes_the_pixel_buds_pro_connect_burst() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        assert_eq!(controller.start(), hex("08 11 00 00"));
        let state = controller.feed(hex(PIXEL_BUDS_PRO_CONNECT)).unwrap();
        let reading = |percent, charging| Some(BatteryReading { percent, charging });
        assert_eq!(state.battery.left, reading(100, true));
        assert_eq!(state.battery.right, reading(100, true));
        assert_eq!(state.battery.charging_case, reading(92, false));
        assert_eq!(state.info.firmware.left.as_deref(), Some("Revision 5"));
        assert_eq!(state.anc, Some(AncMode::Off));
        // Charging in the case: modes supported (0xa8) but none settable.
        assert_eq!(state.anc_availability, AncAvailability::NotNow);
        let caps = controller.capabilities();
        assert_eq!(
            caps.anc_modes,
            [
                AncMode::Off,
                AncMode::NoiseCancelling,
                AncMode::Transparency
            ]
        );
        assert!(controller.encode_anc(AncMode::NoiseCancelling).is_err());
        assert!(caps.has_charging_case);
        assert_eq!(caps.form_factor, FormFactor::Stemmed);
    }

    #[test]
    fn settable_modes_follow_notifications() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        assert!(
            controller.encode_anc(AncMode::Off).is_err(),
            "not reported yet"
        );
        controller.feed(hex("08 13 00 04 01 a8 a8 08")).unwrap();
        assert_eq!(
            controller.capabilities().anc_modes,
            [
                AncMode::Off,
                AncMode::NoiseCancelling,
                AncMode::Transparency
            ]
        );
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        assert_eq!(
            controller.state().anc_availability,
            AncAvailability::Available
        );
        // The probe: set the mode that's already active.
        assert_eq!(controller.take_outbound(), hex("08 12 00 04 01 a8 a8 08"));
        // Set keeps the version and mode masks, changing only the mode.
        assert_eq!(
            controller.encode_anc(AncMode::Transparency).unwrap(),
            hex("08 12 00 04 01 a8 a8 80")
        );
        // Only Off and Transparency settable now; all three still shown.
        controller.feed(hex("08 13 00 04 01 a8 a0 80")).unwrap();
        assert_eq!(controller.capabilities().anc_modes.len(), 3);
        assert!(controller.encode_anc(AncMode::NoiseCancelling).is_err());
        assert!(controller.encode_anc(AncMode::Off).is_ok());
    }

    #[test]
    fn adaptive_shows_no_mode() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        controller.feed(hex("08 13 00 04 01 e8 e8 40")).unwrap();
        assert_eq!(controller.state().anc, None);
        assert!(controller.feed(hex("ff 01 00 02 08 12")).is_ok());
    }

    #[test]
    fn a_refused_probe_makes_noise_control_read_only() {
        // Captured from Pixel Buds Pro (2026-11), worn: an unsigned set is
        // refused with NAK reason 0 ("not supported").
        let controller = BarBudsController::new(DeviceModel::FastPair);
        controller.feed(hex("08 13 00 04 01 a8 a8 08")).unwrap();
        assert_eq!(controller.take_outbound(), hex("08 12 00 04 01 a8 a8 08"));
        let state = controller.feed(hex("ff 02 00 03 00 08 12")).unwrap();
        assert_eq!(state.anc_availability, AncAvailability::OnDeviceOnly);
        assert_eq!(state.anc, Some(AncMode::NoiseCancelling));
        assert!(controller.encode_anc(AncMode::Off).is_err());
        // Later notifications (e.g. a gesture) still update the mode, and
        // it stays read-only for the rest of the connection.
        let state = controller.feed(hex("08 13 00 04 01 a8 a8 80")).unwrap();
        assert_eq!(state.anc, Some(AncMode::Transparency));
        assert_eq!(state.anc_availability, AncAvailability::OnDeviceOnly);
        // Only one probe per connection.
        assert!(controller.take_outbound().is_empty());
        // A new connection tries again.
        controller.reset();
        controller.start();
        controller.feed(hex("08 13 00 04 01 a8 a8 08")).unwrap();
        assert_eq!(
            controller.state().anc_availability,
            AncAvailability::Available
        );
        assert!(!controller.take_outbound().is_empty());
    }

    #[test]
    fn probes_wait_for_a_settable_mode_and_other_naks_are_harmless() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        // In the case: nothing settable, so no probe yet.
        controller.feed(hex("08 13 00 04 01 a8 00 20")).unwrap();
        assert!(controller.take_outbound().is_empty());
        controller.feed(hex("08 13 00 04 01 a8 a8 20")).unwrap();
        assert_eq!(controller.take_outbound(), hex("08 12 00 04 01 a8 a8 20"));
        // "Redundant" (4) or "busy" (1) aren't refusals.
        for reason in ["04", "01"] {
            let nak = hex(&format!("ff 02 00 03 {reason} 08 12"));
            assert_eq!(
                controller.feed(nak).unwrap().anc_availability,
                AncAvailability::Available
            );
        }
    }

    #[test]
    fn one_battery_means_headphones() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        let state = controller.feed(hex("03 03 00 01 d0")).unwrap();
        assert_eq!(
            state.battery.left,
            Some(BatteryReading {
                percent: 80,
                charging: true
            })
        );
        let caps = controller.capabilities();
        assert_eq!(caps.form_factor, FormFactor::Headphones);
        assert!(!caps.has_charging_case);
    }

    #[test]
    fn unknown_levels_and_split_delivery() {
        let controller = BarBudsController::new(DeviceModel::FastPair);
        controller.feed(hex("03 03 00")).unwrap();
        let state = controller.feed(hex("03 3c 7f ff")).unwrap();
        assert_eq!(state.battery.left.unwrap().percent, 60);
        assert_eq!(state.battery.right, None);
        assert_eq!(state.battery.charging_case, None);
    }

    #[test]
    fn ringing_shares_the_channel() {
        use crate::{DeviceSimulator, FindController, RingState};
        // The app passes every chunk to both the driver and FindController,
        // and writes both their outputs to the one channel.
        let controller = BarBudsController::new(DeviceModel::FastPair);
        let find = FindController::new();
        let sim = DeviceSimulator::new(DeviceModel::FastPair);
        let chunk = sim.receive(controller.start());
        controller.feed(chunk.clone()).unwrap();
        find.feed(chunk).unwrap();

        let chunk = sim.receive(find.ring(true, false));
        let before = controller.state();
        assert_eq!(
            controller.feed(chunk.clone()).unwrap(),
            before,
            "ring traffic is ignored"
        );
        assert_eq!(
            find.feed(chunk).unwrap(),
            RingState {
                left: true,
                right: false
            }
        );
        // Noise-control traffic doesn't confuse FindController either.
        let chunk = sim.receive(controller.encode_anc(AncMode::Off).unwrap());
        assert_eq!(
            controller.feed(chunk.clone()).unwrap().anc,
            Some(AncMode::Off)
        );
        assert!(find.feed(chunk).unwrap().left);
    }

    #[test]
    fn a_device_without_earbud_batteries_is_one_piece() {
        // A JBL Tune 770NC sent nothing on connect, not even its battery.
        let controller = BarBudsController::new(DeviceModel::FastPair);
        controller.start();
        let caps = controller.capabilities();
        assert_eq!(caps.form_factor, FormFactor::Headphones);
        assert!(!caps.has_charging_case);
        // A left/right/case report makes it earbuds.
        controller.feed(hex("03 03 00 03 50 50 7f")).unwrap();
        assert_eq!(controller.capabilities().form_factor, FormFactor::Stemmed);
    }
}
