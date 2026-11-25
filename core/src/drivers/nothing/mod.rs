//! Nothing and CMF earbuds and headphones, via the Nothing X control
//! protocol they share. Which model is connected is asked for on connect
//! (see [`models`]); until it answers, only the basics are shown.
//!
//! The control channel is RFCOMM, advertised under [`SERVICE_UUID`]. Every
//! frame, in both directions (multi-byte fields little-endian):
//! ```text
//! 0x55 | control u16 | command u16 | length u16 | sequence u8 | payload | [CRC-16]
//! ```
//! - `control`: bit 0x20 means a CRC-16/MODBUS trailer follows; bits 0–4 of
//!   a reply are an error code (0 = OK).
//! - `command`: low byte = what, high byte = direction: `0xC0` get, `0xF0`
//!   set, `0x40` reply to a get, `0x70` acknowledgement of a set, `0x60`/`0xE0`
//!   unsolicited notification.
//!
//! Received CRCs are not checked: models differ in what they checksum (CMF
//! Buds Pro 2 cover only the payload), and RFCOMM already guarantees
//! integrity. Gadgetbridge, widely used with Nothing earbuds, doesn't check
//! them either.
//!
//! On connect BarBuds sends the requests a real Ear (open) is known to handle
//! (battery, firmware, EQ). The model comes from the Bluetooth name when it's
//! the factory one; otherwise, once the EQ reply arrives, BarBuds asks for
//! the model ID (BudsLink falls back the other way round, after 2 seconds,
//! which suggests some models don't answer). Then it reads that model's other
//! settings one at a time: each answer (or rejection) sends the next, so a
//! request a model ignores only stops the reads after it.
//!
//! Sources: Gadgetbridge (AGPL-3.0) and EarA-linux (GPL-3.0,
//! <https://github.com/oneydef/EarA-linux>) for the framing, battery, EQ and
//! the Custom EQ encoding; BudsLink (GPL-3.0,
//! <https://github.com/maniacx/BudsLink>) for preset codes, noise control,
//! in-ear detection, Dual Connection, Spatial Audio, gestures, model IDs and
//! each model's features. Facts only, no code.
//!
//! Confirmed on Ear (open) hardware (a tester, 2026-11): framing and request
//! CRCs, battery, firmware, EQ presets, the Custom EQ curve, Low Latency,
//! Dual Connection and gesture controls. Everything else, including the
//! model-ID request and all other models, is untested.

pub(crate) mod models;
pub(crate) mod sim;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::error::BarBudsError;
use crate::model::{
    AncMode, BatteryReading, BatteryStatus, BudSide, ComponentStrings, ControlAction,
    ControlCapabilities, ControlGesture, ControlInput, ControlSetting, ControlSlot,
    DeviceCapabilities, EqCapabilities, EqPreset, EqSettings, EqUnit, FeatureToggle, FormFactor,
    SpatialAudioMode,
};
use models::NothingModel;

/// RFCOMM service UUID of the Nothing X control channel (all Nothing/CMF models).
pub(crate) const SERVICE_UUID: &str = "aeac4a03-dff5-498f-843a-34487cf133eb";

const SOF: u8 = 0x55;
const HEADER_LEN: usize = 8;
const CRC_LEN: usize = 2;
/// Control word for host frames: CRC present (0x20), sequence used (0x40),
/// device type 1 = true-wireless earbuds (0x100). As EarA-linux sends.
const HOST_CONTROL: u16 = 0x0160;
const CONTROL_HAS_CRC: u16 = 0x20;
const CONTROL_ERROR: u16 = 0x1F;
/// Longest payloads seen are well under this; larger means lost sync.
const MAX_PAYLOAD: usize = 1024;

mod direction {
    pub const GET: u8 = 0xC0;
    pub const SET: u8 = 0xF0;
    pub const REPLY: u8 = 0x40;
    pub const ACK: u8 = 0x70;
    pub const NOTIFY: u8 = 0x60;
    pub const NOTIFY_ALT: u8 = 0xE0;
}

mod command {
    /// Battery notification (unsolicited).
    pub const BATTERY_NOTIFY: u8 = 0x01;
    /// Noise-control notification (direction `0xE0`; shares its code with
    /// the gestures set, direction `0xF0`).
    pub const ANC_NOTIFY: u8 = 0x03;
    /// In-ear detection: set `[1, 1, on]`.
    pub const IN_EAR_SET: u8 = 0x04;
    /// Lines of `component,type,value`; type 4 is the serial number.
    pub const DEVICE_INFO: u8 = 0x06;
    pub const BATTERY: u8 = 0x07;
    /// In-ear detection: the reply's third byte is 1 when on.
    pub const IN_EAR_GET: u8 = 0x0E;
    /// Noise control: set `[1, mode, 0]`.
    pub const ANC_SET: u8 = 0x0F;
    pub const EQ_SET: u8 = 0x10;
    /// The model ID, e.g. `[0x74, 0xB1]` for `0xB174`.
    pub const MODEL_ID: u8 = 0x1C;
    /// EQ presets on CMF earbuds: set `[code, 0]`.
    pub const LISTENING_SET: u8 = 0x1D;
    /// Noise control: `[kind, value, 0]` triples, kind 1 being the mode.
    pub const ANC_GET: u8 = 0x1E;
    pub const EQ_GET: u8 = 0x1F;
    /// CMF earbuds may report their preset under this code.
    pub const LISTENING_GET: u8 = 0x50;
    /// Low-latency ("Low Lag") mode: set `[1 = on / 2 = off, 0]`.
    pub const LATENCY_SET: u8 = 0x40;
    /// Low-latency mode: the reply's first byte is 1 when on.
    pub const LATENCY_GET: u8 = 0x41;
    pub const FIRMWARE: u8 = 0x42;
    /// Dual Connection (multipoint): set `[1 = on / 0 = off]`.
    pub const DUAL_SET: u8 = 0x1A;
    /// Dual Connection: the reply's first byte is 1 when on.
    pub const DUAL_GET: u8 = 0x27;
    /// Dual Connection changed (direction `0xE0`; shares its code with the
    /// device-info reply, direction `0x40`).
    pub const DUAL_NOTIFY: u8 = 0x06;
    /// Spatial audio: `[mode, variant]`, see [`super::SPATIAL_MODES`]. Some
    /// CMF earbuds reply with one byte (0 off, 1 fixed, 2 head tracking).
    pub const SPATIAL_GET: u8 = 0x4F;
    /// Spatial audio: set `[mode, variant]`.
    pub const SPATIAL_SET: u8 = 0x52;
    /// The Custom EQ curve; see [`super::custom_eq_payload`].
    pub const CUSTOM_EQ_SET: u8 = 0x41;
    pub const CUSTOM_EQ_GET: u8 = 0x44;
    /// Gesture actions: `[count, (bud, button, gesture, action) × count]`.
    pub const GESTURES_GET: u8 = 0x18;
    /// Changes gestures; same layout as the reply (BudsLink sends one).
    pub const GESTURES_SET: u8 = 0x03;
}

/// Spatial audio wire values, `[mode, variant]` (swift-nothing-ear's).
const SPATIAL_MODES: [(SpatialAudioMode, [u8; 2]); 5] = [
    (SpatialAudioMode::Off, [0x00, 0x00]),
    (SpatialAudioMode::Fixed, [0x01, 0x00]),
    (SpatialAudioMode::HeadTracking, [0x01, 0x01]),
    (SpatialAudioMode::Concert, [0x02, 0x00]),
    (SpatialAudioMode::Cinema, [0x03, 0x00]),
];

/// The spatial audio mode in a reply.
fn spatial_mode(payload: &[u8]) -> Option<SpatialAudioMode> {
    match *payload {
        // One byte, as some CMF earbuds report it.
        [single] => match single {
            0x00 => Some(SpatialAudioMode::Off),
            0x01 => Some(SpatialAudioMode::Fixed),
            0x02 => Some(SpatialAudioMode::HeadTracking),
            _ => None,
        },
        [mode, variant, ..] => SPATIAL_MODES
            .iter()
            .find(|&&(_, bytes)| bytes == [mode, variant])
            .map(|&(m, _)| m),
        [] => None,
    }
}

/// The button on each bud that the gestures belong to.
const GESTURE_BUTTON: u8 = 0x01;

/// Gesture wire codes (BudsLink's).
const GESTURES: [(ControlGesture, u8); 5] = [
    (ControlGesture::Single, 0x01),
    (ControlGesture::Double, 0x02),
    (ControlGesture::Triple, 0x03),
    (ControlGesture::Hold, 0x07),
    (ControlGesture::DoubleHold, 0x09),
];

/// Action wire codes (BudsLink's). Noise control is sent as `0x0A`; models
/// also report `0x14`–`0x16` for it (variants of which modes it cycles).
const ACTIONS: [(ControlAction, u8); 8] = [
    (ControlAction::NoAction, 0x01),
    (ControlAction::PlayPause, 0x02),
    (ControlAction::SkipBack, 0x08),
    (ControlAction::SkipForward, 0x09),
    (ControlAction::NoiseControl, 0x0A),
    (ControlAction::VoiceAssistant, 0x0B),
    (ControlAction::VolumeUp, 0x12),
    (ControlAction::VolumeDown, 0x13),
];
const NOISE_CONTROL_VARIANTS: [u8; 3] = [0x14, 0x15, 0x16];

fn gesture_code(gesture: ControlGesture) -> Option<u8> {
    GESTURES
        .iter()
        .find(|&&(g, _)| g == gesture)
        .map(|&(_, c)| c)
}

fn action_code(action: ControlAction) -> Option<u8> {
    ACTIONS.iter().find(|&&(a, _)| a == action).map(|&(_, c)| c)
}

fn action_for(code: u8) -> Option<ControlAction> {
    if NOISE_CONTROL_VARIANTS.contains(&code) {
        return Some(ControlAction::NoiseControl);
    }
    ACTIONS.iter().find(|&&(_, c)| c == code).map(|&(a, _)| a)
}

fn side_code(side: BudSide) -> u8 {
    match side {
        BudSide::Left => component::LEFT,
        BudSide::Right => component::RIGHT,
    }
}

/// Gesture settings from a reply; entries BarBuds doesn't know (other
/// buttons, headphone controls, other actions) are skipped.
pub(crate) fn gesture_settings(payload: &[u8]) -> Result<Vec<ControlSetting>, BarBudsError> {
    let (&count, entries) = payload
        .split_first()
        .ok_or_else(|| BarBudsError::malformed("empty gestures payload"))?;
    Ok(entries
        .as_chunks::<4>()
        .0
        .iter()
        .take(count as usize)
        .filter_map(|entry| {
            let side = match entry[0] {
                component::LEFT => BudSide::Left,
                component::RIGHT => BudSide::Right,
                _ => return None,
            };
            if entry[1] != GESTURE_BUTTON {
                return None;
            }
            let gesture = GESTURES.iter().find(|&&(_, c)| c == entry[2])?.0;
            Some(ControlSetting {
                side,
                gesture,
                action: action_for(entry[3])?,
            })
        })
        .collect())
}

/// One gesture entry as sent or reported.
pub(crate) fn gesture_entry(setting: ControlSetting) -> Option<[u8; 4]> {
    Some([
        side_code(setting.side),
        GESTURE_BUTTON,
        gesture_code(setting.gesture)?,
        action_code(setting.action)?,
    ])
}

/// The device-info field type holding a serial number.
const INFO_TYPE_SERIAL: &str = "4";

/// Battery component IDs.
mod component {
    /// One level for both buds, or the only battery of headphones.
    pub const BOTH: u8 = 0x01;
    pub const LEFT: u8 = 0x02;
    pub const RIGHT: u8 = 0x03;
    pub const CASE: u8 = 0x04;
}

/// Wire code of the Nothing Custom curve, shown as sliders on models with
/// `custom_sliders` (the Ear (open)).
pub(crate) const CUSTOM_CODE: u8 = 5;

/// Custom EQ bands as shown, Bass, Mid, Treble (dB).
const BAND_LABELS: [&str; 3] = ["Bass", "Mid", "Treble"];
const MAX_GAIN: f32 = 6.0;

/// The Custom EQ payload, as EarA-linux builds it (from Nothing's web tool):
/// a band count, a pre-gain, then three 13-byte filters (gain f32, centre
/// frequency f32, Q f32, type) for mid (980 Hz), treble (3.5 kHz) and bass
/// (140 Hz). All floats little-endian. Only the gains and the pre-gain
/// (minus the largest boost, so boosts can't clip) change.
const CUSTOM_EQ_TEMPLATE: [u8; 53] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x75, 0x44, 0xC3, 0xF5,
    0x28, 0x3F, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x5A, 0x45, 0x00, 0x00, 0x80, 0x3F, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x43, 0xCD, 0xCC, 0x4C, 0x3F, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00,
];
const PRE_GAIN_OFFSET: usize = 1;
/// Offsets of the mid, treble and bass gains, in that order.
const GAIN_OFFSETS: [usize; 3] = [6, 19, 32];

/// `gains` are Bass, Mid, Treble.
pub(crate) fn custom_eq_payload(gains: [f32; 3]) -> Vec<u8> {
    let [bass, mid, treble] = gains;
    let mut payload = CUSTOM_EQ_TEMPLATE.to_vec();
    let boost = bass.max(mid).max(treble);
    // -0.0 when nothing is boosted, as the references send.
    let pre_gain = if boost > 0.0 { -boost } else { -0.0 };
    payload[PRE_GAIN_OFFSET..PRE_GAIN_OFFSET + 4].copy_from_slice(&pre_gain.to_le_bytes());
    for (offset, gain) in GAIN_OFFSETS.into_iter().zip([mid, treble, bass]) {
        payload[offset..offset + 4].copy_from_slice(&gain.to_le_bytes());
    }
    payload
}

/// Bass, Mid, Treble from a Custom EQ payload.
pub(crate) fn custom_eq_gains(payload: &[u8]) -> Result<[f32; 3], BarBudsError> {
    let gain = |offset: usize| -> Result<f32, BarBudsError> {
        let bytes = payload
            .get(offset..offset + 4)
            .ok_or_else(|| BarBudsError::malformed("short custom EQ payload"))?;
        let value = f32::from_le_bytes(bytes.try_into().expect("4 bytes"));
        Ok(if value.is_finite() {
            value.clamp(-MAX_GAIN, MAX_GAIN)
        } else {
            0.0
        })
    };
    let [mid, treble, bass] = [
        gain(GAIN_OFFSETS[0])?,
        gain(GAIN_OFFSETS[1])?,
        gain(GAIN_OFFSETS[2])?,
    ];
    Ok([bass, mid, treble])
}

/// The noise-control mode in an ANC reply or notification: the value of the
/// `[kind, value, 0]` triple with kind 1, as EarA-linux reads it, else the
/// second byte, as BudsLink does.
fn anc_code(payload: &[u8]) -> Option<u8> {
    payload
        .chunks(3)
        .find(|triple| triple.len() >= 2 && triple[0] == 1)
        .map(|triple| triple[1])
        .or_else(|| payload.get(1).copied())
}

/// CRC-16/MODBUS: reflected polynomial 0xA001, initial value 0xFFFF.
pub(crate) fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn encode_frame(control: u16, command: u16, sequence: u8, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(payload.len()).expect("control payloads are small");
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len() + CRC_LEN);
    frame.push(SOF);
    frame.extend_from_slice(&control.to_le_bytes());
    frame.extend_from_slice(&command.to_le_bytes());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(sequence);
    frame.extend_from_slice(payload);
    if control & CONTROL_HAS_CRC != 0 {
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_le_bytes());
    }
    frame
}

fn battery_status(payload: &[u8]) -> Result<BatteryStatus, BarBudsError> {
    let (&count, pairs) = payload
        .split_first()
        .ok_or_else(|| BarBudsError::malformed("empty battery payload"))?;
    let mut status = BatteryStatus::default();
    for pair in pairs.as_chunks::<2>().0.iter().take(count as usize) {
        let level = pair[1] & 0x7F;
        if level > 100 {
            continue; // Not reporting.
        }
        let reading = Some(BatteryReading {
            percent: level,
            charging: pair[1] & 0x80 != 0,
        });
        match pair[0] {
            component::BOTH => {
                status.left = reading;
                status.right = reading;
            }
            component::LEFT => status.left = reading,
            component::RIGHT => status.right = reading,
            component::CASE => status.charging_case = reading,
            _ => {}
        }
    }
    Ok(status)
}

/// Serial numbers from the device-info lines (`component,type,value`).
fn serial_numbers(payload: &[u8]) -> ComponentStrings {
    let text = String::from_utf8_lossy(payload);
    let mut serials = ComponentStrings::default();
    for line in text.lines() {
        let mut fields = line.trim().splitn(3, ',');
        let (Some(component), Some(kind), Some(value)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let value = value.trim();
        if kind != INFO_TYPE_SERIAL || value.is_empty() {
            continue;
        }
        let slot = match component.parse::<u8>() {
            Ok(component::LEFT) => &mut serials.left,
            Ok(component::RIGHT) => &mut serials.right,
            Ok(component::CASE) => &mut serials.charging_case,
            _ => continue,
        };
        *slot = Some(value.to_string());
    }
    serials
}

/// What the model-ID request found out.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// Not answered yet.
    Asking,
    Known(&'static NothingModel),
    /// An ID BarBuds doesn't know, or no answer: only the basics.
    Unknown,
}

pub(crate) struct NothingDriver {
    /// Frame sequence number, as the Nothing X app sends.
    sequence: u8,
    /// Settings sent but not yet acknowledged, oldest first: the set
    /// command and the state change its acknowledgement confirms.
    pending: std::collections::VecDeque<(u8, Event)>,
    /// Reads sent after the start-up replies, one at a time (see the module docs).
    follow_ups: std::collections::VecDeque<u8>,
    /// The follow-up read awaiting an answer.
    awaiting: Option<u8>,
    identity: Identity,
    /// The EQ code last reported, kept until the model is known.
    eq_code: Option<u8>,
    /// The active preset's ID, `None` for the Custom curve.
    preset: Option<String>,
    /// The Custom curve: Bass, Mid, Treble.
    custom_gains: [f32; 3],
    /// The Noise Cancelling strength last reported, reused when switching
    /// back to Noise Cancelling.
    cancelling_code: Option<u8>,
    /// The model the Bluetooth name gave, kept across `start()`.
    named_model: Option<&'static NothingModel>,
}

impl Default for NothingDriver {
    fn default() -> Self {
        Self {
            sequence: 1,
            pending: Default::default(),
            follow_ups: Default::default(),
            awaiting: None,
            identity: Identity::Asking,
            eq_code: None,
            preset: None,
            custom_gains: [0.0; 3],
            cancelling_code: None,
            named_model: None,
        }
    }
}

impl NothingDriver {
    fn model(&self) -> Option<&'static NothingModel> {
        match self.identity {
            Identity::Known(model) => Some(model),
            _ => None,
        }
    }

    fn frame(&mut self, direction: u8, command: u8, payload: &[u8]) -> Vec<u8> {
        let sequence = self.sequence;
        // Gadgetbridge wraps before 0xFD; keep to the same range.
        self.sequence = if sequence >= 0xFC { 1 } else { sequence + 1 };
        let command = u16::from_le_bytes([command, direction]);
        encode_frame(HOST_CONTROL, command, sequence, payload)
    }

    /// Records the model (or that it's unknown) and queues its reads.
    fn identify(&mut self, identity: Identity) {
        self.identity = identity;
        // Low Latency is common to every model.
        self.follow_ups.push_back(command::LATENCY_GET);
        if let Identity::Known(model) = identity {
            let reads = [
                (model.dual_connection, command::DUAL_GET),
                (model.in_ear_detection, command::IN_EAR_GET),
                (!model.spatial_audio.is_empty(), command::SPATIAL_GET),
                (model.anc.is_some(), command::ANC_GET),
                (model.custom_sliders, command::CUSTOM_EQ_GET),
                (!model.gestures.is_empty(), command::GESTURES_GET),
            ];
            self.follow_ups.extend(
                reads
                    .into_iter()
                    .filter(|&(on, _)| on)
                    .map(|(_, read)| read),
            );
        }
    }

    /// The change confirmed by an acknowledgement of `command`, if one is waiting.
    fn take_pending(&mut self, command: u8) -> Option<Event> {
        let index = self.pending.iter().position(|&(c, _)| c == command)?;
        let event = self.pending.remove(index).map(|(_, event)| event)?;
        if let Event::Eq(eq) = &event {
            self.preset = eq.preset.clone();
            if let Ok(gains) = <[f32; 3]>::try_from(eq.gains.as_slice()) {
                self.custom_gains = gains;
            }
        }
        Some(event)
    }

    /// The EQ as shown: the active preset, and on models with sliders the
    /// Custom curve.
    fn eq(&self, preset: Option<String>) -> EqSettings {
        let sliders = self.model().is_some_and(|m| m.custom_sliders);
        EqSettings {
            preset,
            gains: if sliders {
                self.custom_gains.to_vec()
            } else {
                vec![]
            },
        }
    }

    /// The EQ event for the last reported code, once the model is known.
    fn eq_event(&mut self) -> Result<Option<Event>, BarBudsError> {
        let (Some(model), Some(code)) = (self.model(), self.eq_code) else {
            return Ok(None);
        };
        self.preset = if model.custom_sliders && code == CUSTOM_CODE {
            None
        } else {
            let &(id, _, _) = model
                .presets
                .iter()
                .find(|&&(_, _, c)| c == code)
                .ok_or_else(|| BarBudsError::malformed(format!("unknown EQ preset {code}")))?;
            Some(id.to_string())
        };
        Ok(Some(Event::Eq(self.eq(self.preset.clone()))))
    }

    /// Sends the next follow-up read once the previous one was answered.
    fn send_follow_up(&mut self, outbound: &mut Vec<u8>) {
        if self.awaiting.is_none()
            && let Some(command) = self.follow_ups.pop_front()
        {
            self.awaiting = Some(command);
            outbound.extend(self.frame(direction::GET, command, &[]));
        }
    }

    fn set(&mut self, command: u8, payload: &[u8], confirms: Event) -> Vec<u8> {
        self.pending.push_back((command, confirms));
        self.frame(direction::SET, command, payload)
    }

    fn anc_mode(&mut self, code: u8) -> Option<AncMode> {
        let anc = self.model()?.anc?;
        if code == anc.off {
            Some(AncMode::Off)
        } else if Some(code) == anc.transparency {
            Some(AncMode::Transparency)
        } else if anc.cancelling.contains(&code) {
            self.cancelling_code = Some(code);
            Some(AncMode::NoiseCancelling)
        } else {
            None
        }
    }

    fn handle(
        &mut self,
        direction: u8,
        command: u8,
        payload: &[u8],
    ) -> Result<Option<Event>, BarBudsError> {
        let notify = matches!(direction, direction::NOTIFY | direction::NOTIFY_ALT);
        match (direction, command) {
            (direction::REPLY, command::BATTERY) => {
                battery_status(payload).map(|b| Some(Event::Battery(b)))
            }
            (_, command::BATTERY_NOTIFY) if notify => {
                battery_status(payload).map(|b| Some(Event::Battery(b)))
            }
            (direction::REPLY, command::FIRMWARE) => {
                let version = String::from_utf8_lossy(payload)
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
            (direction::REPLY, command::MODEL_ID) => {
                let &[low, high, ..] = payload else {
                    self.identify(Identity::Unknown);
                    return Err(BarBudsError::malformed("short model ID"));
                };
                let id = u16::from_le_bytes([low, high]);
                self.identify(models::model(id).map_or(Identity::Unknown, Identity::Known));
                // The EQ code may have arrived before the model was known.
                self.eq_event()
            }
            (direction::REPLY, command::EQ_GET | command::LISTENING_GET) => {
                let &code = payload
                    .first()
                    .ok_or_else(|| BarBudsError::malformed("empty EQ payload"))?;
                self.eq_code = Some(code);
                self.eq_event()
            }
            (direction::REPLY, command::ANC_GET) | (direction::NOTIFY_ALT, command::ANC_NOTIFY) => {
                let code = anc_code(payload)
                    .ok_or_else(|| BarBudsError::malformed("empty noise-control payload"))?;
                Ok(Some(Event::Anc(self.anc_mode(code))))
            }
            (direction::REPLY, command::IN_EAR_GET) => {
                let &value = payload
                    .get(2)
                    .ok_or_else(|| BarBudsError::malformed("short in-ear payload"))?;
                Ok(Some(Event::Toggle(
                    FeatureToggle::InEarDetection,
                    value == 1,
                )))
            }
            (direction::REPLY, command::GESTURES_GET) => {
                let settings = gesture_settings(payload)?;
                Ok((!settings.is_empty()).then_some(Event::Controls(settings)))
            }
            (direction::REPLY, command::CUSTOM_EQ_GET) => {
                self.custom_gains = custom_eq_gains(payload)?;
                Ok(Some(Event::Eq(self.eq(self.preset.clone()))))
            }
            (direction::REPLY, command::DUAL_GET)
            | (direction::NOTIFY_ALT, command::DUAL_NOTIFY) => {
                let &value = payload
                    .first()
                    .ok_or_else(|| BarBudsError::malformed("empty dual-connection payload"))?;
                Ok(Some(Event::Toggle(FeatureToggle::Multipoint, value == 1)))
            }
            (direction::REPLY, command::SPATIAL_GET) => {
                let mode = spatial_mode(payload).ok_or_else(|| {
                    BarBudsError::malformed(format!("unknown spatial audio mode {payload:02x?}"))
                })?;
                Ok(Some(Event::SpatialAudio(mode)))
            }
            (direction::REPLY, command::LATENCY_GET) => {
                let &value = payload
                    .first()
                    .ok_or_else(|| BarBudsError::malformed("empty low-latency payload"))?;
                Ok(Some(Event::Toggle(FeatureToggle::LowLatency, value == 1)))
            }
            (direction::REPLY, command::DEVICE_INFO) => {
                let serials = serial_numbers(payload);
                let any = serials.left.is_some()
                    || serials.right.is_some()
                    || serials.charging_case.is_some();
                Ok(any.then_some(Event::SerialNumbers(serials)))
            }
            (direction::ACK, command) => Ok(self.take_pending(command)),
            _ => Ok(None), // Acks and notifications BarBuds doesn't show.
        }
    }
}

impl EarbudDriver for NothingDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        let Some(model) = self.model() else {
            // Until (or unless) the model is known: the basics.
            return DeviceCapabilities {
                display_name: "Nothing".to_string(),
                anc_modes: vec![],
                eq: None,
                has_charging_case: true,
                form_factor: FormFactor::StemmedInEar,
                toggles: if matches!(self.identity, Identity::Unknown) {
                    vec![FeatureToggle::LowLatency]
                } else {
                    vec![]
                },
                can_ring: true,
                has_balance: false,
                anc_cycle_modes: vec![],
                spatial_audio_modes: vec![],
                controls: None,
            };
        };
        let anc_modes = model.anc.map_or(vec![], |anc| {
            let mut modes = vec![AncMode::Off, AncMode::NoiseCancelling];
            if anc.transparency.is_some() {
                modes.push(AncMode::Transparency);
            }
            modes
        });
        let toggles = [
            (model.in_ear_detection, FeatureToggle::InEarDetection),
            (model.dual_connection, FeatureToggle::Multipoint),
            (true, FeatureToggle::LowLatency),
        ]
        .into_iter()
        .filter(|&(on, _)| on)
        .map(|(_, toggle)| toggle)
        .collect();
        let controls = (!model.gestures.is_empty()).then(|| ControlCapabilities {
            input: ControlInput::Pinch,
            slots: model
                .gestures
                .iter()
                .map(|slot| ControlSlot {
                    side: slot.side,
                    gesture: slot.gesture,
                    actions: slot.actions.to_vec(),
                })
                .collect(),
        });
        let bands = if model.custom_sliders {
            BAND_LABELS.iter().map(|l| l.to_string()).collect()
        } else {
            vec![]
        };
        DeviceCapabilities {
            display_name: model.name.to_string(),
            anc_modes,
            eq: Some(EqCapabilities {
                band_labels: bands,
                unit: EqUnit::Decibels,
                min_gain: -MAX_GAIN,
                max_gain: MAX_GAIN,
                step: 0.5,
                presets: model
                    .presets
                    .iter()
                    .map(|&(id, name, _)| EqPreset {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                    .collect(),
            }),
            has_charging_case: !model.single_battery,
            form_factor: model.form_factor,
            toggles,
            // Over the Fast Pair channel; confirmed on the Ear (open).
            can_ring: true,
            has_balance: false,
            anc_cycle_modes: vec![],
            spatial_audio_modes: model.spatial_audio.to_vec(),
            controls,
        }
    }

    fn set_device_name(&mut self, name: &str) {
        self.named_model = models::model_for_name(name);
    }

    fn start(&mut self) -> Vec<u8> {
        *self = Self {
            named_model: self.named_model,
            ..Self::default()
        };
        // What a real Ear (open) is known to handle at connect time; the
        // model's reads (and the model ID, unless the name gave the model)
        // follow the EQ reply.
        match self.named_model {
            Some(model) => self.identify(Identity::Known(model)),
            None => self.follow_ups.push_back(command::MODEL_ID),
        }
        [command::BATTERY, command::FIRMWARE, command::EQ_GET]
            .into_iter()
            .flat_map(|cmd| self.frame(direction::GET, cmd, &[]))
            .collect()
    }

    fn parse_frame(&mut self, buf: &[u8], outbound: &mut Vec<u8>) -> Option<Decoded> {
        let &first = buf.first()?;
        if first != SOF {
            let skip = buf.iter().position(|&b| b == SOF).unwrap_or(buf.len());
            let err =
                BarBudsError::malformed(format!("skipped {skip} byte(s) before start of frame"));
            return Some(Decoded {
                consumed: skip,
                result: Err(err),
            });
        }
        if buf.len() < HEADER_LEN {
            return None;
        }
        let control = u16::from_le_bytes([buf[1], buf[2]]);
        let (command, direction) = (buf[3], buf[4]);
        let len = u16::from_le_bytes([buf[5], buf[6]]) as usize;
        if len > MAX_PAYLOAD {
            let err = BarBudsError::malformed(format!("{len}-byte payload"));
            return Some(Decoded {
                consumed: 1,
                result: Err(err),
            });
        }
        let crc_len = if control & CONTROL_HAS_CRC != 0 {
            CRC_LEN
        } else {
            0
        };
        let total = HEADER_LEN + len + crc_len;
        if buf.len() < total {
            return None;
        }
        let payload = &buf[HEADER_LEN..HEADER_LEN + len];
        // The EQ reply starts the follow-up reads; each answer (or
        // rejection) of one sends the next.
        let answered = direction == direction::REPLY
            && (command == command::EQ_GET || Some(command) == self.awaiting);
        if answered && Some(command) == self.awaiting {
            self.awaiting = None;
        }
        let result = match control & CONTROL_ERROR {
            0 => self.handle(direction, command, payload),
            code => {
                if direction == direction::ACK {
                    self.take_pending(command);
                }
                if direction == direction::REPLY && command == command::MODEL_ID {
                    self.identify(Identity::Unknown);
                }
                Err(BarBudsError::device(format!(
                    "earbuds rejected command {:#04x} (error {code})",
                    command
                )))
            }
        };
        // After handling: a model-ID reply queues that model's reads.
        if answered {
            self.send_follow_up(outbound);
        }
        Some(Decoded {
            consumed: total,
            result,
        })
    }

    fn encode_anc(&mut self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        let anc = self
            .model()
            .and_then(|m| m.anc)
            .ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("ANC mode {mode:?}"),
            })?;
        let code = match mode {
            AncMode::Off => anc.off,
            AncMode::Transparency => anc.transparency.ok_or_else(|| BarBudsError::Unsupported {
                feature: "transparency".to_string(),
            })?,
            // The strength the buds last used, else the model's default.
            AncMode::NoiseCancelling => self
                .cancelling_code
                .filter(|c| anc.cancelling.contains(c))
                .unwrap_or(anc.cancelling[0]),
        };
        Ok(self.set(
            command::ANC_SET,
            &[0x01, code, 0x00],
            Event::Anc(Some(mode)),
        ))
    }

    fn encode_eq(&mut self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let model = self.model().ok_or_else(|| BarBudsError::Unsupported {
            feature: "EQ".to_string(),
        })?;
        let Some(preset) = &eq.preset else {
            if !model.custom_sliders {
                return Err(BarBudsError::Unsupported {
                    feature: "a custom EQ curve".to_string(),
                });
            }
            // The Custom curve: select it if needed, then write it.
            let gains = <[f32; 3]>::try_from(eq.gains.as_slice()).map_err(|_| {
                BarBudsError::invalid_argument(format!("{} EQ gains, expected 3", eq.gains.len()))
            })?;
            if gains.iter().any(|g| !(-MAX_GAIN..=MAX_GAIN).contains(g)) {
                return Err(BarBudsError::invalid_argument(format!(
                    "EQ gains {gains:?} outside ±{MAX_GAIN} dB"
                )));
            }
            let confirms = Event::Eq(EqSettings {
                preset: None,
                gains: gains.to_vec(),
            });
            let mut frames = Vec::new();
            if self.preset.is_some() {
                frames = self.set(command::EQ_SET, &[CUSTOM_CODE, 0x00], confirms.clone());
            }
            frames.extend(self.set(command::CUSTOM_EQ_SET, &custom_eq_payload(gains), confirms));
            return Ok(frames);
        };
        let &(id, _, code) = model
            .presets
            .iter()
            .find(|&&(id, _, _)| preset == id)
            .ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("EQ preset {preset:?}"),
            })?;
        let set = if model.listening_mode {
            command::LISTENING_SET
        } else {
            command::EQ_SET
        };
        let confirms = Event::Eq(self.eq(Some(id.to_string())));
        Ok(self.set(set, &[code, 0x00], confirms))
    }

    fn encode_spatial_audio(&mut self, mode: SpatialAudioMode) -> Result<Vec<u8>, BarBudsError> {
        let &(_, bytes) = SPATIAL_MODES
            .iter()
            .find(|&&(m, _)| m == mode)
            .expect("every mode has wire values");
        Ok(self.set(command::SPATIAL_SET, &bytes, Event::SpatialAudio(mode)))
    }

    fn encode_control(&mut self, setting: ControlSetting) -> Result<Vec<u8>, BarBudsError> {
        let entry = gesture_entry(setting).ok_or_else(|| BarBudsError::Unsupported {
            feature: format!("{setting:?}"),
        })?;
        let mut payload = vec![1];
        payload.extend(entry);
        Ok(self.set(
            command::GESTURES_SET,
            &payload,
            Event::Controls(vec![setting]),
        ))
    }

    fn encode_toggle(&mut self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        match toggle {
            FeatureToggle::LowLatency => {
                let value = if on { 1 } else { 2 };
                Ok(self.set(
                    command::LATENCY_SET,
                    &[value, 0x00],
                    Event::Toggle(toggle, on),
                ))
            }
            FeatureToggle::Multipoint => Ok(self.set(
                command::DUAL_SET,
                &[u8::from(on)],
                Event::Toggle(toggle, on),
            )),
            FeatureToggle::InEarDetection => Ok(self.set(
                command::IN_EAR_SET,
                &[0x01, 0x01, u8::from(on)],
                Event::Toggle(toggle, on),
            )),
            _ => Err(BarBudsError::Unsupported {
                feature: format!("{toggle:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::BarBudsController;
    use crate::model::DeviceModel;

    const EAR_OPEN: u16 = 0xB174;

    /// A controller for `id` that has answered the model-ID request (its
    /// follow-up reads are queued, not sent).
    fn identified(id: u16) -> std::sync::Arc<BarBudsController> {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller
            .feed(reply(
                direction::REPLY,
                command::MODEL_ID,
                &id.to_le_bytes(),
            ))
            .unwrap();
        controller
    }

    fn ear_open() -> std::sync::Arc<BarBudsController> {
        identified(EAR_OPEN)
    }

    fn ear_open_driver() -> NothingDriver {
        NothingDriver {
            identity: Identity::Known(models::model(EAR_OPEN).unwrap()),
            ..Default::default()
        }
    }

    fn preset_eq(id: &str) -> EqSettings {
        EqSettings {
            preset: Some(id.to_string()),
            gains: vec![0.0; 3],
        }
    }

    fn custom_eq(gains: [f32; 3]) -> EqSettings {
        EqSettings {
            preset: None,
            gains: gains.to_vec(),
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    /// A reply frame as the earbuds would send it (with a full-frame CRC).
    fn reply(direction: u8, command: u8, payload: &[u8]) -> Vec<u8> {
        encode_frame(
            HOST_CONTROL,
            u16::from_le_bytes([command, direction]),
            0x20,
            payload,
        )
    }

    #[test]
    fn crc16_matches_modbus_check_value() {
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn encodes_reference_frame() {
        // A host frame from Gadgetbridge's Nothing tests (an EQ write on
        // another model, command 0xF01D): checks header layout and CRC.
        assert_eq!(
            encode_frame(0x0160, 0xF01D, 0x14, &[0x02, 0x00]),
            hex("55 60 01 1d f0 02 00 14 02 00 eb cd")
        );
    }

    #[test]
    fn decodes_captured_battery_reply() {
        // Captured from CMF Buds Pro 2 (EarA-linux tests): left and right 95%.
        // Its CRC covers only the payload, which must not stop it decoding.
        let controller = ear_open();
        controller
            .feed(hex("55 60 01 07 40 05 00 02 02 02 5f 03 5f 2c a2"))
            .unwrap();
        let battery = controller.state().battery;
        let ninety_five = Some(BatteryReading {
            percent: 95,
            charging: false,
        });
        assert_eq!(battery.left, ninety_five);
        assert_eq!(battery.right, ninety_five);
        assert_eq!(battery.charging_case, None);
    }

    #[test]
    fn battery_variants() {
        // Case charging at 60%; one shared level for both buds.
        let status = battery_status(&[2, component::BOTH, 80, component::CASE, 0x80 | 60]).unwrap();
        assert_eq!(
            status.left,
            Some(BatteryReading {
                percent: 80,
                charging: false
            })
        );
        assert_eq!(status.right, status.left);
        assert_eq!(
            status.charging_case,
            Some(BatteryReading {
                percent: 60,
                charging: true
            })
        );
        // 0x7F = not reporting.
        assert_eq!(
            battery_status(&[1, component::LEFT, 0x7F]).unwrap().left,
            None
        );
    }

    #[test]
    fn start_requests_battery_firmware_and_eq() {
        let mut driver = ear_open_driver();
        let frames = driver.start();
        assert_eq!(&frames[..8], &hex("55 60 01 07 c0 00 00 01")[..]);
        assert_eq!(frames.len(), 3 * (HEADER_LEN + CRC_LEN));
        assert_eq!(frames[13..15], [command::FIRMWARE, direction::GET]);
        assert_eq!(frames[23..25], [command::EQ_GET, direction::GET]);
    }

    #[test]
    fn the_model_is_asked_for_after_the_eq_reply() {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::BATTERY, &[0]))
            .unwrap();
        assert!(controller.take_outbound().is_empty());
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[3, 0]))
            .unwrap();
        let request = controller.take_outbound();
        assert_eq!(request[3..5], [command::MODEL_ID, direction::GET]);
        assert_eq!(request[7], 4); // sequence: after the three start-up requests
        // Presets aren't known until the model is.
        assert_eq!(controller.state().eq, None);
        assert_eq!(controller.capabilities().display_name, "Nothing");
        // Only once, even if the EQ is reported again.
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[3, 0]))
            .unwrap();
        assert!(controller.take_outbound().is_empty());

        // The answer names the model, shows the EQ reported earlier and
        // starts the model's own reads.
        controller
            .feed(reply(direction::REPLY, command::MODEL_ID, &[0x74, 0xB1]))
            .unwrap();
        assert_eq!(controller.capabilities().display_name, "Nothing Ear (open)");
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-bass")
        );
        assert_eq!(controller.take_outbound()[3], command::LATENCY_GET);
        controller
            .feed(reply(direction::REPLY, command::LATENCY_GET, &[1]))
            .unwrap();
        assert!(controller.state().toggles[&FeatureToggle::LowLatency]);
    }

    #[test]
    fn tester_capture_codes() {
        // From a real Ear (open) (firmware 1.0.1.28): the EQ reply carried 3,
        // which the Nothing X app showed as More Bass.
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(hex("55 60 01 1f 40 01 00 03 03 9d e6"))
            .unwrap();
        controller
            .feed(reply(direction::REPLY, command::MODEL_ID, &[0x74, 0xB1]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-bass")
        );
        // Its acknowledgement of a preset change, no error.
        controller.encode_eq(preset_eq("voice")).unwrap();
        controller
            .feed(hex("55 60 01 10 70 00 00 04 3e 1f"))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("voice")
        );
    }

    #[test]
    fn eq_preset_applies_after_acknowledgement() {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0]))
            .unwrap();
        controller
            .feed(reply(direction::REPLY, command::MODEL_ID, &[0x74, 0xB1]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("balanced")
        );

        let frame = controller.encode_eq(preset_eq("more-bass")).unwrap();
        // Set EQ, More Bass (3), sequence 6 (after the three start-up
        // requests, the model ID and the low-latency request).
        assert_eq!(frame[..10], hex("55 60 01 10 f0 02 00 06 03 00")[..]);
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("balanced")
        );
        controller
            .feed(reply(direction::ACK, command::EQ_SET, &[]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-bass")
        );
    }

    #[test]
    fn rejected_eq_keeps_state() {
        let controller = ear_open();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[2]))
            .unwrap();
        controller.encode_eq(preset_eq("voice")).unwrap();
        let mut ack = reply(direction::ACK, command::EQ_SET, &[]);
        ack[1] |= 0x03; // error code 3
        assert!(matches!(
            controller.feed(ack),
            Err(BarBudsError::Device { .. })
        ));
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-treble")
        );
    }

    #[test]
    fn firmware_version() {
        let controller = ear_open();
        controller
            .feed(reply(direction::REPLY, command::FIRMWARE, b"1.0.2.3\0"))
            .unwrap();
        assert_eq!(
            controller.state().info.firmware.left.as_deref(),
            Some("1.0.2.3")
        );
    }

    #[test]
    fn handles_split_frames_and_garbage() {
        let controller = ear_open();
        // Garbage before a frame is skipped and reported once...
        assert!(controller.feed(vec![0x00, 0x13]).is_err());
        // ...and a frame split into single bytes still decodes.
        let frame = reply(
            direction::NOTIFY_ALT,
            command::BATTERY_NOTIFY,
            &[1, component::CASE, 42],
        );
        for byte in frame {
            controller.feed(vec![byte]).unwrap();
        }
        assert_eq!(
            controller.state().battery.charging_case.unwrap().percent,
            42
        );
    }

    /// Feeds one frame to the driver directly (Low Lag Mode isn't in the
    /// capabilities, so the controller would refuse it).
    fn driver_feed(driver: &mut NothingDriver, frame: &[u8]) -> Option<Event> {
        driver
            .parse_frame(frame, &mut Vec::new())
            .unwrap()
            .result
            .unwrap()
    }

    #[test]
    fn low_latency_encoding() {
        let mut driver = ear_open_driver();
        assert!(matches!(
            driver_feed(
                &mut driver,
                &reply(direction::REPLY, command::LATENCY_GET, &[2])
            ),
            Some(Event::Toggle(FeatureToggle::LowLatency, false))
        ));
        let on = driver
            .encode_toggle(FeatureToggle::LowLatency, true)
            .unwrap();
        // Set 0x40 with [1 = on, 0], as EarA-linux sends.
        assert_eq!(on[3..5], [command::LATENCY_SET, direction::SET]);
        assert_eq!(on[8..10], [1, 0]);
        assert!(matches!(
            driver_feed(
                &mut driver,
                &reply(direction::ACK, command::LATENCY_SET, &[])
            ),
            Some(Event::Toggle(FeatureToggle::LowLatency, true))
        ));
        let off = driver
            .encode_toggle(FeatureToggle::LowLatency, false)
            .unwrap();
        assert_eq!(off[8..10], [2, 0]);
    }

    #[test]
    fn acknowledgements_confirm_the_matching_setting() {
        let mut driver = ear_open_driver();
        driver.encode_eq(&preset_eq("voice")).unwrap();
        driver
            .encode_toggle(FeatureToggle::LowLatency, true)
            .unwrap();
        // The low-latency ack arrives first; it must not confirm the EQ.
        assert!(matches!(
            driver_feed(
                &mut driver,
                &reply(direction::ACK, command::LATENCY_SET, &[])
            ),
            Some(Event::Toggle(FeatureToggle::LowLatency, true))
        ));
        assert!(matches!(
            driver_feed(&mut driver, &reply(direction::ACK, command::EQ_SET, &[])),
            Some(Event::Eq(eq)) if eq.preset.as_deref() == Some("voice")
        ));
    }

    #[test]
    fn serial_numbers_from_device_info() {
        // EarA-linux's documented layout (`component,type,value`, type 4 =
        // serial), with made-up serials.
        let controller = ear_open();
        let info = b"2,4,SIMLEFT01\n3,4,SIMRIGHT01\n4,4,SIMCASE01\n2,1,ignored\n";
        controller
            .feed(reply(direction::REPLY, command::DEVICE_INFO, info))
            .unwrap();
        let serials = controller.state().info.serial_numbers;
        assert_eq!(serials.left.as_deref(), Some("SIMLEFT01"));
        assert_eq!(serials.right.as_deref(), Some("SIMRIGHT01"));
        assert_eq!(serials.charging_case.as_deref(), Some("SIMCASE01"));
    }

    #[test]
    fn nothing_preset_names() {
        let caps = ear_open_driver().capabilities().eq.unwrap();
        let names: Vec<_> = caps.presets.iter().map(|p| p.name.as_str()).collect();
        // "Custom" is added by the app for the sliders.
        assert_eq!(names, ["Balanced", "More Bass", "More Treble", "Voice"]);
        assert_eq!(caps.band_labels, ["Bass", "Mid", "Treble"]);
    }

    #[test]
    fn no_anc_and_ear_hook_shape() {
        let controller = ear_open();
        let caps = controller.capabilities();
        assert!(caps.anc_modes.is_empty());
        assert_eq!(caps.form_factor, FormFactor::EarHook);
        assert!(matches!(
            controller.encode_anc(AncMode::Off),
            Err(BarBudsError::Unsupported { .. })
        ));
    }

    #[test]
    fn custom_eq_payload_matches_eara_linux() {
        // EarA-linux's encoding (from Nothing's web tool) of bass +2, mid 0,
        // treble -1: pre-gain -2 (the largest boost), gains in mid, treble,
        // bass order, all little-endian floats.
        let payload = custom_eq_payload([2.0, 0.0, -1.0]);
        assert_eq!(payload.len(), 53);
        assert_eq!(payload[..6], hex("03 00 00 00 c0 01")[..]);
        assert_eq!(payload[6..10], hex("00 00 00 00")[..]); // mid 0
        assert_eq!(payload[19..23], hex("00 00 80 bf")[..]); // treble -1
        assert_eq!(payload[32..36], hex("00 00 00 40")[..]); // bass +2
        assert_eq!(custom_eq_gains(&payload).unwrap(), [2.0, 0.0, -1.0]);
        // Nothing boosted: pre-gain -0.0, as the references send.
        assert_eq!(
            custom_eq_payload([-3.0, 0.0, 0.0])[1..5],
            hex("00 00 00 80")[..]
        );
    }

    #[test]
    fn follow_up_reads_go_one_at_a_time() {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        let mut sent = Vec::new();
        for (cmd, payload) in [
            (command::EQ_GET, vec![CUSTOM_CODE, 0]),
            (command::MODEL_ID, vec![0x74, 0xB1]),
            (command::LATENCY_GET, vec![2]),
            (command::DUAL_GET, vec![1]),
            (command::CUSTOM_EQ_GET, custom_eq_payload([1.5, -2.0, 3.0])),
        ] {
            controller
                .feed(reply(direction::REPLY, cmd, &payload))
                .unwrap();
            let out = controller.take_outbound();
            if !out.is_empty() {
                sent.push(out[3]);
            }
        }
        assert_eq!(
            sent,
            [
                command::MODEL_ID,
                command::LATENCY_GET,
                command::DUAL_GET,
                command::CUSTOM_EQ_GET,
                command::GESTURES_GET,
            ]
        );
        let state = controller.state();
        assert_eq!(state.eq, Some(custom_eq([1.5, -2.0, 3.0])));
        assert!(state.toggles[&FeatureToggle::Multipoint]);
        assert!(!state.toggles[&FeatureToggle::LowLatency]);
    }

    #[test]
    fn a_rejected_follow_up_moves_on() {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        controller.take_outbound(); // Model ID
        controller
            .feed(reply(direction::REPLY, command::MODEL_ID, &[0x74, 0xB1]))
            .unwrap();
        controller.take_outbound(); // Low latency
        let mut rejected = reply(direction::REPLY, command::LATENCY_GET, &[]);
        rejected[1] |= 0x03; // error code 3
        assert!(controller.feed(rejected).is_err());
        assert_eq!(controller.take_outbound()[3], command::DUAL_GET);
    }

    #[test]
    fn custom_curve_selects_custom_then_writes_it() {
        let controller = ear_open();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        controller.take_outbound();
        let frames = controller.encode_eq(custom_eq([3.0, 0.0, 0.0])).unwrap();
        // Set EQ to Custom (5), then the curve.
        assert_eq!(frames[3..5], [command::EQ_SET, direction::SET]);
        assert_eq!(frames[8..10], [CUSTOM_CODE, 0]);
        let second = &frames[HEADER_LEN + 2 + CRC_LEN..];
        assert_eq!(second[3..5], [command::CUSTOM_EQ_SET, direction::SET]);
        assert_eq!(second[5..7], [53, 0]);
        // Once on Custom, only the curve is written.
        controller
            .feed(reply(direction::ACK, command::EQ_SET, &[]))
            .unwrap();
        controller
            .feed(reply(direction::ACK, command::CUSTOM_EQ_SET, &[]))
            .unwrap();
        assert_eq!(controller.state().eq, Some(custom_eq([3.0, 0.0, 0.0])));
        let next = controller.encode_eq(custom_eq([3.0, 1.0, 0.0])).unwrap();
        assert_eq!(next[3], command::CUSTOM_EQ_SET);
        // Out of range is refused.
        assert!(controller.encode_eq(custom_eq([7.0, 0.0, 0.0])).is_err());
    }

    #[test]
    fn presets_keep_the_custom_sliders() {
        let controller = ear_open();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[5, 0]))
            .unwrap();
        let curve = custom_eq_payload([2.0, 0.0, 0.0]);
        controller
            .feed(reply(direction::REPLY, command::CUSTOM_EQ_GET, &curve))
            .unwrap();
        controller.encode_eq(preset_eq("voice")).unwrap();
        controller
            .feed(reply(direction::ACK, command::EQ_SET, &[]))
            .unwrap();
        let eq = controller.state().eq.unwrap();
        assert_eq!(eq.preset.as_deref(), Some("voice"));
        assert_eq!(eq.gains, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn dual_connection_reads_writes_and_follows_notifications() {
        let controller = ear_open();
        let frame = controller
            .encode_toggle(FeatureToggle::Multipoint, false)
            .unwrap();
        assert_eq!(frame[3..5], [command::DUAL_SET, direction::SET]);
        assert_eq!(frame[8], 0);
        controller
            .feed(reply(direction::ACK, command::DUAL_SET, &[]))
            .unwrap();
        assert!(!controller.state().toggles[&FeatureToggle::Multipoint]);
        controller
            .feed(reply(direction::NOTIFY_ALT, command::DUAL_NOTIFY, &[1]))
            .unwrap();
        assert!(controller.state().toggles[&FeatureToggle::Multipoint]);
    }

    /// BudsLink's Ear (open) factory defaults, as a gestures reply payload.
    const DEFAULT_GESTURES: &str = "08 02 01 02 09 03 01 02 09 02 01 03 08 03 01 03 08 02 01 07 13 03 01 07 12 02 01 09 01 03 01 09 01";

    #[test]
    fn decodes_default_gestures() {
        let controller = ear_open();
        controller
            .feed(reply(
                direction::REPLY,
                command::GESTURES_GET,
                &hex(DEFAULT_GESTURES),
            ))
            .unwrap();
        let controls = controller.state().controls;
        assert_eq!(controls.len(), 8);
        let action = |side, gesture| {
            controls
                .iter()
                .find(|c| c.side == side && c.gesture == gesture)
                .unwrap()
                .action
        };
        use ControlAction::*;
        use ControlGesture::*;
        assert_eq!(action(BudSide::Left, Double), SkipForward);
        assert_eq!(action(BudSide::Right, Triple), SkipBack);
        assert_eq!(action(BudSide::Left, Hold), VolumeDown);
        assert_eq!(action(BudSide::Right, Hold), VolumeUp);
        assert_eq!(action(BudSide::Right, DoubleHold), NoAction);
        // Every reported action is one the capabilities offer for its slot.
        let caps = controller.capabilities().controls.unwrap();
        for c in &controls {
            let slot = caps
                .slots
                .iter()
                .find(|s| s.side == c.side && s.gesture == c.gesture)
                .unwrap();
            assert!(slot.actions.contains(&c.action), "{c:?}");
        }
    }

    #[test]
    fn unknown_gesture_entries_are_skipped() {
        // An unknown action (0x7f) and an unknown bud (0x09) next to a known entry.
        let payload = hex("03 02 01 02 7f 09 01 02 09 03 01 03 0b");
        let settings = gesture_settings(&payload).unwrap();
        assert_eq!(
            settings,
            [ControlSetting {
                side: BudSide::Right,
                gesture: ControlGesture::Triple,
                action: ControlAction::VoiceAssistant,
            }]
        );
    }

    #[test]
    fn changing_a_gesture_applies_after_acknowledgement() {
        let controller = ear_open();
        controller
            .feed(reply(
                direction::REPLY,
                command::GESTURES_GET,
                &hex(DEFAULT_GESTURES),
            ))
            .unwrap();
        let setting = ControlSetting {
            side: BudSide::Left,
            gesture: ControlGesture::Double,
            action: ControlAction::VoiceAssistant,
        };
        let frame = controller.encode_control(setting).unwrap();
        // BudsLink's layout: one entry, [left, button 1, double, assistant].
        assert_eq!(frame[3..5], [command::GESTURES_SET, direction::SET]);
        assert_eq!(frame[8..13], hex("01 02 01 02 0b")[..]);
        assert!(!controller.state().controls.contains(&setting));
        controller
            .feed(reply(direction::ACK, command::GESTURES_SET, &[]))
            .unwrap();
        assert!(controller.state().controls.contains(&setting));
        assert_eq!(controller.state().controls.len(), 8);
    }

    #[test]
    fn actions_a_gesture_lacks_are_refused() {
        let controller = ear_open();
        for setting in [
            // Double pinch can't change the volume.
            (
                BudSide::Left,
                ControlGesture::Double,
                ControlAction::VolumeUp,
            ),
            // Single pinch isn't remappable on the Ear (open).
            (
                BudSide::Left,
                ControlGesture::Single,
                ControlAction::PlayPause,
            ),
        ] {
            let (side, gesture, action) = setting;
            assert!(
                controller
                    .encode_control(ControlSetting {
                        side,
                        gesture,
                        action
                    })
                    .is_err()
            );
        }
    }

    /// Connects a controller to a simulated `model`, running every follow-up read.
    fn connect_to(
        model: &'static NothingModel,
    ) -> (std::sync::Arc<BarBudsController>, sim::NothingSim) {
        use crate::simulator::Simulator;
        let controller = BarBudsController::new(DeviceModel::Nothing);
        let mut device = sim::NothingSim::new(model);
        let mut outbound = controller.start();
        while !outbound.is_empty() {
            controller.feed(device.receive(&outbound)).unwrap();
            outbound = controller.take_outbound();
        }
        (controller, device)
    }

    fn exchange(controller: &BarBudsController, device: &mut sim::NothingSim, frame: Vec<u8>) {
        use crate::simulator::Simulator;
        controller.feed(device.receive(&frame)).unwrap();
    }

    #[test]
    fn the_model_table_is_consistent() {
        let mut ids: Vec<u16> = models::MODELS.iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), models::MODELS.len(), "duplicate model IDs");
        let tested: Vec<&str> = models::MODELS
            .iter()
            .filter(|m| m.tested)
            .map(|m| m.name)
            .collect();
        assert_eq!(tested, ["Nothing Ear (open)"]);
        for model in models::MODELS {
            let mut codes: Vec<u8> = model.presets.iter().map(|&(_, _, c)| c).collect();
            codes.sort();
            codes.dedup();
            assert_eq!(
                codes.len(),
                model.presets.len(),
                "{}: duplicate preset codes",
                model.name
            );
            assert!(!model.presets.is_empty(), "{}", model.name);
            if let Some(anc) = model.anc {
                assert!(!anc.cancelling.is_empty(), "{}", model.name);
            }
            for slot in model.gestures {
                assert!(
                    slot.actions.len() >= 2,
                    "{}: {:?}",
                    model.name,
                    slot.gesture
                );
                for &action in slot.actions {
                    assert!(action_code(action).is_some(), "{}: {action:?}", model.name);
                }
            }
            assert_eq!(
                model.single_battery,
                matches!(
                    model.form_factor,
                    FormFactor::Headphones | FormFactor::Neckband
                )
            );
        }
    }

    #[test]
    fn every_listed_model_connects_and_round_trips() {
        for model in models::MODELS {
            let name = model.name;
            let (controller, mut device) = connect_to(model);
            let caps = controller.capabilities();
            let state = controller.state();
            assert_eq!(caps.display_name, name);
            assert_eq!(caps.form_factor, model.form_factor, "{name}");
            assert_eq!(caps.has_charging_case, !model.single_battery, "{name}");
            assert!(state.battery.left.is_some(), "{name}: battery");
            assert_eq!(
                state.battery.charging_case.is_some(),
                !model.single_battery,
                "{name}"
            );
            assert!(state.info.firmware.left.is_some(), "{name}: firmware");
            for toggle in &caps.toggles {
                assert!(
                    state.toggles.contains_key(toggle),
                    "{name}: {toggle:?} not read"
                );
            }

            // Noise control: reported, and every mode switches.
            if model.anc.is_some() {
                assert_eq!(state.anc, Some(AncMode::NoiseCancelling), "{name}");
                for &mode in &caps.anc_modes {
                    exchange(
                        &controller,
                        &mut device,
                        controller.encode_anc(mode).unwrap(),
                    );
                    assert_eq!(controller.state().anc, Some(mode), "{name}: {mode:?}");
                }
            } else {
                assert!(caps.anc_modes.is_empty(), "{name}");
            }

            // Every preset, with the model's own command (the simulator
            // only accepts that one).
            let eq = caps.eq.clone().unwrap();
            assert_eq!(
                state.eq.as_ref().and_then(|e| e.preset.clone()).as_deref(),
                Some(model.presets[0].0),
                "{name}"
            );
            for preset in &eq.presets {
                let settings = EqSettings {
                    preset: Some(preset.id.clone()),
                    gains: vec![],
                };
                exchange(
                    &controller,
                    &mut device,
                    controller.encode_eq(settings).unwrap(),
                );
                assert_eq!(
                    controller.state().eq.unwrap().preset.as_deref(),
                    Some(preset.id.as_str()),
                    "{name}: {}",
                    preset.name
                );
            }
            assert_eq!(eq.band_labels.is_empty(), !model.custom_sliders, "{name}");

            // Every spatial audio mode.
            assert_eq!(caps.spatial_audio_modes, model.spatial_audio, "{name}");
            if !model.spatial_audio.is_empty() {
                assert_eq!(state.spatial_audio, Some(SpatialAudioMode::Off), "{name}");
            }
            for &mode in &caps.spatial_audio_modes {
                exchange(
                    &controller,
                    &mut device,
                    controller.encode_spatial_audio(mode).unwrap(),
                );
                assert_eq!(
                    controller.state().spatial_audio,
                    Some(mode),
                    "{name}: {mode:?}"
                );
            }

            // Every switch, both ways.
            for &toggle in &caps.toggles {
                for on in [true, false] {
                    exchange(
                        &controller,
                        &mut device,
                        controller.encode_toggle(toggle, on).unwrap(),
                    );
                    assert_eq!(
                        controller.state().toggles[&toggle],
                        on,
                        "{name}: {toggle:?}"
                    );
                }
            }

            // Every gesture action.
            match caps.controls {
                Some(controls) => {
                    assert_eq!(controls.slots.len(), model.gestures.len(), "{name}");
                    for slot in controls.slots {
                        for action in slot.actions {
                            let setting = ControlSetting {
                                side: slot.side,
                                gesture: slot.gesture,
                                action,
                            };
                            exchange(
                                &controller,
                                &mut device,
                                controller.encode_control(setting).unwrap(),
                            );
                            assert!(
                                controller.state().controls.contains(&setting),
                                "{name}: {setting:?}"
                            );
                        }
                    }
                }
                None => assert!(model.gestures.is_empty(), "{name}"),
            }
        }
    }

    #[test]
    fn unknown_or_refused_model_ids_show_the_basics() {
        // An ID BarBuds doesn't list.
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        controller.take_outbound();
        controller
            .feed(reply(direction::REPLY, command::MODEL_ID, &[0x99, 0xB1]))
            .unwrap();
        let caps = controller.capabilities();
        assert_eq!(caps.display_name, "Nothing");
        assert_eq!(caps.eq, None);
        assert!(caps.anc_modes.is_empty());
        assert_eq!(caps.toggles, [FeatureToggle::LowLatency]);
        assert_eq!(controller.take_outbound()[3], command::LATENCY_GET);

        // A refused model-ID request.
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        controller.take_outbound();
        let mut refused = reply(direction::REPLY, command::MODEL_ID, &[]);
        refused[1] |= 0x01;
        assert!(controller.feed(refused).is_err());
        assert_eq!(
            controller.capabilities().toggles,
            [FeatureToggle::LowLatency]
        );
        assert_eq!(controller.take_outbound()[3], command::LATENCY_GET);
    }

    #[test]
    fn cmf_presets_use_the_listening_mode_command() {
        // CMF Buds Pro 2: set with 0xF01D; reported under 0x1F or 0x50.
        let controller = identified(0xB187);
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0x01, 0]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("rock")
        );
        let frame = controller.encode_eq(preset_eq("pop")).unwrap();
        assert_eq!(frame[3..5], [command::LISTENING_SET, direction::SET]);
        assert_eq!(frame[8..10], [0x03, 0x00]);
        controller
            .feed(reply(direction::ACK, command::LISTENING_SET, &[]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("pop")
        );
        controller
            .feed(reply(direction::REPLY, command::LISTENING_GET, &[0x05, 0]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("classical")
        );
        // No sliders: a custom curve is refused.
        assert!(controller.encode_eq(custom_eq([1.0, 0.0, 0.0])).is_err());
    }

    #[test]
    fn noise_cancelling_keeps_its_strength() {
        // Nothing Ear (a): the buds report Mid (0x02); switching away and
        // back sets Mid again, not the default High.
        let controller = identified(0xB183);
        controller
            .feed(reply(direction::REPLY, command::ANC_GET, &[1, 0x02, 0]))
            .unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        let off = controller.encode_anc(AncMode::Off).unwrap();
        assert_eq!(off[3..5], [command::ANC_SET, direction::SET]);
        assert_eq!(off[8..11], [0x01, 0x05, 0x00]);
        let back = controller.encode_anc(AncMode::NoiseCancelling).unwrap();
        assert_eq!(back[8..11], [0x01, 0x02, 0x00]);
        // A notification (direction 0xE0, code 0x03) updates the mode.
        controller
            .feed(reply(
                direction::NOTIFY_ALT,
                command::ANC_NOTIFY,
                &[1, 0x07, 0],
            ))
            .unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::Transparency));
        // Without a reported strength, the model's default (High).
        let fresh = identified(0xB183);
        let frame = fresh.encode_anc(AncMode::NoiseCancelling).unwrap();
        assert_eq!(frame[9], 0x01);
    }

    #[test]
    fn noise_control_gesture_variants_are_one_action() {
        // Models report 0x0A or 0x14-0x16 for noise control.
        let settings = gesture_settings(&hex("02 02 01 07 15 03 01 07 0a")).unwrap();
        assert!(
            settings
                .iter()
                .all(|s| s.action == ControlAction::NoiseControl)
        );
        assert_eq!(settings.len(), 2);
        // Headphone controls (device 0x06) and other buttons are skipped.
        assert!(
            gesture_settings(&hex("02 06 0a 01 0a 02 05 01 0a"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn in_ear_detection_reads_and_writes() {
        let controller = identified(0xB183);
        controller
            .feed(reply(direction::REPLY, command::IN_EAR_GET, &[1, 1, 0]))
            .unwrap();
        assert!(!controller.state().toggles[&FeatureToggle::InEarDetection]);
        let frame = controller
            .encode_toggle(FeatureToggle::InEarDetection, true)
            .unwrap();
        assert_eq!(frame[3..5], [command::IN_EAR_SET, direction::SET]);
        assert_eq!(frame[8..11], [0x01, 0x01, 0x01]);
        // Not offered on the Ear (open).
        assert!(
            ear_open()
                .encode_toggle(FeatureToggle::InEarDetection, true)
                .is_err()
        );
    }

    #[test]
    fn a_factory_name_identifies_the_model_without_asking() {
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.set_device_name("Nothing Ear (open)".to_string());
        assert_eq!(controller.start().len(), 3 * (HEADER_LEN + CRC_LEN));
        assert_eq!(controller.capabilities().display_name, "Nothing Ear (open)");
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[3, 0]))
            .unwrap();
        // Straight to the model's reads: the Ear (open)'s proven sequence.
        assert_eq!(controller.take_outbound()[3], command::LATENCY_GET);
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-bass")
        );
        // Kept across reconnects.
        controller.reset();
        controller.start();
        assert_eq!(controller.capabilities().display_name, "Nothing Ear (open)");
    }

    #[test]
    fn names_match_the_longest_factory_name() {
        let name = |n: &str| models::model_for_name(n).map(|m| m.name);
        assert_eq!(name("CMF Buds Pro 2"), Some("CMF Buds Pro 2"));
        assert_eq!(name("CMF Buds Pro"), Some("CMF Buds Pro"));
        assert_eq!(name("CMF Buds 2 Plus"), Some("CMF Buds 2 Plus"));
        assert_eq!(name("CMF Buds 2a"), Some("CMF Buds 2a"));
        assert_eq!(name("nothing ear (1)"), Some("Nothing Ear (1)"));
        assert_eq!(name("Nothing Ear"), Some("Nothing Ear"));
        assert_eq!(name("Nothing Ear (a)"), Some("Nothing Ear (a)"));
        // Renamed: ask the earbuds instead.
        assert_eq!(name("Nikhil's buds"), None);
        let controller = BarBudsController::new(DeviceModel::Nothing);
        controller.set_device_name("Nikhil's buds".to_string());
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        assert_eq!(controller.take_outbound()[3], command::MODEL_ID);
    }

    #[test]
    fn spatial_audio_modes_read_and_write() {
        // The reply payloads from swift-nothing-ear's tests: CMF Buds Pro 2
        // in Fixed (one byte), CMF Headphone Pro in Cinema (two bytes). Their
        // frames leave out the CRC the header announces, so they're framed
        // here with one.
        let buds = identified(0xB187);
        assert_eq!(
            buds.capabilities().spatial_audio_modes,
            [SpatialAudioMode::Off, SpatialAudioMode::Fixed]
        );
        buds.feed(reply(direction::REPLY, command::SPATIAL_GET, &[0x01]))
            .unwrap();
        assert_eq!(buds.state().spatial_audio, Some(SpatialAudioMode::Fixed));
        let headphones = identified(0xB175);
        headphones
            .feed(reply(direction::REPLY, command::SPATIAL_GET, &[0x03, 0x00]))
            .unwrap();
        assert_eq!(
            headphones.state().spatial_audio,
            Some(SpatialAudioMode::Cinema)
        );

        // Its write: [mode, variant], e.g. Concert = 02 00.
        let frame = headphones
            .encode_spatial_audio(SpatialAudioMode::Concert)
            .unwrap();
        assert_eq!(frame[3..5], [command::SPATIAL_SET, direction::SET]);
        assert_eq!(frame[8..10], [0x02, 0x00]);
        headphones
            .feed(reply(direction::ACK, command::SPATIAL_SET, &[]))
            .unwrap();
        assert_eq!(
            headphones.state().spatial_audio,
            Some(SpatialAudioMode::Concert)
        );

        // Head tracking only on the Headphone (1): 01 01.
        let headphone_1 = identified(0xB170);
        let frame = headphone_1
            .encode_spatial_audio(SpatialAudioMode::HeadTracking)
            .unwrap();
        assert_eq!(frame[8..10], [0x01, 0x01]);
        assert!(
            buds.encode_spatial_audio(SpatialAudioMode::HeadTracking)
                .is_err()
        );
        assert!(ear_open().capabilities().spatial_audio_modes.is_empty());
        // swift-nothing-ear's write for CMF Buds Pro 2 Fixed, whole frame.
        assert_eq!(
            encode_frame(HOST_CONTROL, 0xF052, 0x01, &[0x01, 0x00]),
            hex("55 60 01 52 f0 02 00 01 01 00 44 3d")
        );
    }
}
