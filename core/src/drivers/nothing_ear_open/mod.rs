//! Nothing Ear (open), via the Nothing X control protocol shared by Nothing
//! and CMF earbuds.
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
//! Buds Pro 2 cover only the payload), the Ear (open)'s scheme is unconfirmed,
//! and RFCOMM already guarantees integrity. Gadgetbridge, widely used with
//! Nothing earbuds, doesn't check them either.
//!
//! Protocol facts come from Gadgetbridge (AGPL-3.0; facts only, no code) and
//! EarA-linux (GPL-3.0, <https://github.com/oneydef/EarA-linux>), whose Ear
//! (open) profile lists no ANC and no in-ear detection, and EQ presets
//! Balanced, More Bass, More Treble and Voice plus Custom.
//!
//! Confirmed on Ear (open) hardware (a tester, 2026-11): framing and request
//! CRCs, battery, firmware, EQ presets (codes from BudsLink, GPL-3.0,
//! <https://github.com/maniacx/BudsLink>; facts only) and low-latency mode.
//! Also confirmed: the Custom EQ curve (EarA-linux's encoding) and Dual
//! Connection (BudsLink's commands). Not yet confirmed: gesture controls
//! (BudsLink's Ear (open) table).

pub(crate) mod sim;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::error::BarBudsError;
use crate::model::{
    AncMode, BatteryReading, BatteryStatus, BudSide, ComponentStrings, ControlAction,
    ControlCapabilities, ControlGesture, ControlInput, ControlSetting, ControlSlot,
    DeviceCapabilities, EqCapabilities, EqPreset, EqSettings, EqUnit, FeatureToggle, FormFactor,
};

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
    /// Lines of `component,type,value`; type 4 is the serial number.
    pub const DEVICE_INFO: u8 = 0x06;
    pub const BATTERY: u8 = 0x07;
    pub const EQ_SET: u8 = 0x10;
    pub const EQ_GET: u8 = 0x1F;
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
    /// The Custom EQ curve; see [`super::custom_eq_payload`].
    pub const CUSTOM_EQ_SET: u8 = 0x41;
    pub const CUSTOM_EQ_GET: u8 = 0x44;
    /// Gesture actions: `[count, (bud, button, gesture, action) × count]`.
    pub const GESTURES_GET: u8 = 0x18;
    /// Changes gestures; same layout as the reply (BudsLink sends one).
    pub const GESTURES_SET: u8 = 0x03;
}

/// The Ear (open)'s only button, on each bud.
const GESTURE_BUTTON: u8 = 0x01;

/// Gesture wire codes, from BudsLink's Ear (open) table.
const GESTURES: [(ControlGesture, u8); 4] = [
    (ControlGesture::Double, 0x02),
    (ControlGesture::Triple, 0x03),
    (ControlGesture::Hold, 0x07),
    (ControlGesture::DoubleHold, 0x09),
];

/// Action wire codes, from BudsLink's Nothing table.
const ACTIONS: [(ControlAction, u8); 6] = [
    (ControlAction::NoAction, 0x01),
    (ControlAction::SkipBack, 0x08),
    (ControlAction::SkipForward, 0x09),
    (ControlAction::VoiceAssistant, 0x0B),
    (ControlAction::VolumeUp, 0x12),
    (ControlAction::VolumeDown, 0x13),
];

/// The actions the Nothing X app offers for each gesture (BudsLink's table).
fn gesture_actions(gesture: ControlGesture) -> Vec<ControlAction> {
    use ControlAction::*;
    match gesture {
        ControlGesture::Double | ControlGesture::Triple => {
            vec![SkipBack, SkipForward, VoiceAssistant]
        }
        ControlGesture::Hold => vec![VolumeUp, VolumeDown, VoiceAssistant],
        ControlGesture::DoubleHold => vec![VolumeUp, VolumeDown, VoiceAssistant, NoAction],
        ControlGesture::Single => vec![],
    }
}

fn gesture_code(gesture: ControlGesture) -> Option<u8> {
    GESTURES
        .iter()
        .find(|&&(g, _)| g == gesture)
        .map(|&(_, c)| c)
}

fn action_code(action: ControlAction) -> Option<u8> {
    ACTIONS.iter().find(|&&(a, _)| a == action).map(|&(_, c)| c)
}

fn side_code(side: BudSide) -> u8 {
    match side {
        BudSide::Left => component::LEFT,
        BudSide::Right => component::RIGHT,
    }
}

/// Gesture settings from a reply; entries BarBuds doesn't know are skipped.
pub(crate) fn gesture_settings(payload: &[u8]) -> Result<Vec<ControlSetting>, BarBudsError> {
    let (&count, entries) = payload
        .split_first()
        .ok_or_else(|| BarBudsError::malformed("empty gestures payload"))?;
    Ok(entries
        .chunks_exact(4)
        .take(count as usize)
        .filter_map(|entry| {
            let side = match entry[0] {
                component::LEFT => BudSide::Left,
                component::RIGHT => BudSide::Right,
                _ => return None,
            };
            let gesture = GESTURES.iter().find(|&&(_, c)| c == entry[2])?.0;
            let action = ACTIONS.iter().find(|&&(_, c)| c == entry[3])?.0;
            Some(ControlSetting {
                side,
                gesture,
                action,
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
    /// One level for both buds.
    pub const BOTH: u8 = 0x01;
    pub const LEFT: u8 = 0x02;
    pub const RIGHT: u8 = 0x03;
    pub const CASE: u8 = 0x04;
}

/// The Ear (open)'s presets as the Nothing X app names them: `(id, name,
/// wire code)`, as in BudsLink. Confirmed on hardware (2026-11): Balanced,
/// Voice, More Treble and More Bass each select the same preset in the app
/// (EarA-linux's table, which swaps 1 and 3, does not).
pub(crate) const PRESETS: [(&str, &str, u8); 4] = [
    ("balanced", "Balanced", 0),
    ("more-bass", "More Bass", 3),
    ("more-treble", "More Treble", 2),
    ("voice", "Voice", 1),
];
/// Wire code of the Custom curve (BudsLink's), shown as the sliders.
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
    for pair in pairs.chunks_exact(2).take(count as usize) {
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

pub(crate) struct NothingEarOpenDriver {
    /// Frame sequence number, as the Nothing X app sends.
    sequence: u8,
    /// Settings sent but not yet acknowledged, oldest first: the set
    /// command and the state change its acknowledgement confirms.
    pending: std::collections::VecDeque<(u8, Event)>,
    /// Reads sent after the start-up replies, one at a time (see `start`).
    follow_ups: std::collections::VecDeque<u8>,
    /// The follow-up read awaiting an answer.
    awaiting: Option<u8>,
    /// The active preset's ID, `None` for the Custom curve.
    preset: Option<String>,
    /// The Custom curve: Bass, Mid, Treble.
    custom_gains: [f32; 3],
}

impl Default for NothingEarOpenDriver {
    fn default() -> Self {
        Self {
            sequence: 1,
            pending: Default::default(),
            follow_ups: Default::default(),
            awaiting: None,
            preset: None,
            custom_gains: [0.0; 3],
        }
    }
}

impl NothingEarOpenDriver {
    fn frame(&mut self, direction: u8, command: u8, payload: &[u8]) -> Vec<u8> {
        let sequence = self.sequence;
        // Gadgetbridge wraps before 0xFD; keep to the same range.
        self.sequence = if sequence >= 0xFC { 1 } else { sequence + 1 };
        let command = u16::from_le_bytes([command, direction]);
        encode_frame(HOST_CONTROL, command, sequence, payload)
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

    /// The EQ as shown: the active preset, and the Custom curve's sliders.
    fn eq(&self, preset: Option<String>) -> EqSettings {
        EqSettings {
            preset,
            gains: self.custom_gains.to_vec(),
        }
    }

    fn preset_eq(&self, id: &str) -> EqSettings {
        self.eq(Some(id.to_string()))
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
            (direction::REPLY, command::EQ_GET) => {
                let &code = payload
                    .first()
                    .ok_or_else(|| BarBudsError::malformed("empty EQ payload"))?;
                self.preset = if code == CUSTOM_CODE {
                    None
                } else {
                    let &(id, _, _) =
                        PRESETS
                            .iter()
                            .find(|&&(_, _, c)| c == code)
                            .ok_or_else(|| {
                                BarBudsError::malformed(format!("unknown EQ preset {code}"))
                            })?;
                    Some(id.to_string())
                };
                Ok(Some(Event::Eq(self.eq(self.preset.clone()))))
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

impl EarbudDriver for NothingEarOpenDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            display_name: "Nothing Ear (open)".to_string(),
            anc_modes: vec![],
            eq: Some(EqCapabilities {
                // The sliders are the Custom curve ("Custom" in the picker).
                band_labels: BAND_LABELS.iter().map(|l| l.to_string()).collect(),
                unit: EqUnit::Decibels,
                min_gain: -MAX_GAIN,
                max_gain: MAX_GAIN,
                step: 0.5,
                presets: PRESETS
                    .iter()
                    .map(|&(id, name, _)| EqPreset {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                    .collect(),
            }),
            has_charging_case: true,
            form_factor: FormFactor::EarHook,
            toggles: vec![FeatureToggle::Multipoint, FeatureToggle::LowLatency],
            // Confirmed on hardware: the Ear (open) accepts the Fast Pair ring.
            can_ring: true,
            has_balance: false,
            anc_cycle_modes: vec![],
            controls: Some(ControlCapabilities {
                input: ControlInput::Pinch,
                slots: [BudSide::Left, BudSide::Right]
                    .into_iter()
                    .flat_map(|side| {
                        GESTURES.iter().map(move |&(gesture, _)| ControlSlot {
                            side,
                            gesture,
                            actions: gesture_actions(gesture),
                        })
                    })
                    .collect(),
            }),
        }
    }

    fn start(&mut self) -> Vec<u8> {
        *self = Self::default();
        // What a real Ear (open) is known to handle at connect time. Other
        // reads follow one at a time once the EQ reply arrives, so they
        // can't disturb these.
        self.follow_ups.extend([
            command::LATENCY_GET,
            command::DUAL_GET,
            command::CUSTOM_EQ_GET,
            command::GESTURES_GET,
        ]);
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
        if direction == direction::REPLY
            && (command == command::EQ_GET || Some(command) == self.awaiting)
        {
            if Some(command) == self.awaiting {
                self.awaiting = None;
            }
            self.send_follow_up(outbound);
        }
        let result = match control & CONTROL_ERROR {
            0 => self.handle(direction, command, payload),
            code => {
                if direction == direction::ACK {
                    self.take_pending(command);
                }
                Err(BarBudsError::device(format!(
                    "earbuds rejected command {:#04x} (error {code})",
                    command
                )))
            }
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
        let Some(preset) = &eq.preset else {
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
        let &(id, _, code) = PRESETS
            .iter()
            .find(|&&(id, _, _)| preset == id)
            .ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("EQ preset {preset:?}"),
            })?;
        let confirms = Event::Eq(self.preset_eq(id));
        Ok(self.set(command::EQ_SET, &[code, 0x00], confirms))
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let mut driver = NothingEarOpenDriver::default();
        let frames = driver.start();
        assert_eq!(&frames[..8], &hex("55 60 01 07 c0 00 00 01")[..]);
        assert_eq!(frames.len(), 3 * (HEADER_LEN + CRC_LEN));
        assert_eq!(frames[13..15], [command::FIRMWARE, direction::GET]);
        assert_eq!(frames[23..25], [command::EQ_GET, direction::GET]);
    }

    #[test]
    fn low_latency_is_requested_after_the_eq_reply() {
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::BATTERY, &[0]))
            .unwrap();
        assert!(controller.take_outbound().is_empty());
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[3, 0]))
            .unwrap();
        let request = controller.take_outbound();
        assert_eq!(request[3..5], [command::LATENCY_GET, direction::GET]);
        assert_eq!(request[7], 4); // sequence: after the three start-up requests
        // Only once, even if the EQ is reported again.
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[3, 0]))
            .unwrap();
        assert!(controller.take_outbound().is_empty());
        controller
            .feed(reply(direction::REPLY, command::LATENCY_GET, &[1]))
            .unwrap();
        assert!(controller.state().toggles[&FeatureToggle::LowLatency]);
    }

    #[test]
    fn tester_capture_codes() {
        // From a real Ear (open) (firmware 1.0.1.28): the EQ reply carried 3,
        // which the Nothing X app showed as More Bass.
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
        controller.start();
        controller
            .feed(hex("55 60 01 1f 40 01 00 03 03 9d e6"))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("more-bass")
        );
        // Its acknowledgement of a preset change, sequence 4, no error.
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("balanced")
        );

        let frame = controller.encode_eq(preset_eq("more-bass")).unwrap();
        // Set EQ, More Bass (3), sequence 5 (after the three start-up
        // requests and the low-latency request).
        assert_eq!(frame[..10], hex("55 60 01 10 f0 02 00 05 03 00")[..]);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
    fn driver_feed(driver: &mut NothingEarOpenDriver, frame: &[u8]) -> Option<Event> {
        driver
            .parse_frame(frame, &mut Vec::new())
            .unwrap()
            .result
            .unwrap()
    }

    #[test]
    fn low_latency_encoding() {
        let mut driver = NothingEarOpenDriver::default();
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
        let mut driver = NothingEarOpenDriver::default();
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let caps = NothingEarOpenDriver::default().capabilities().eq.unwrap();
        let names: Vec<_> = caps.presets.iter().map(|p| p.name.as_str()).collect();
        // "Custom" is added by the app for the sliders.
        assert_eq!(names, ["Balanced", "More Bass", "More Treble", "Voice"]);
        assert_eq!(caps.band_labels, ["Bass", "Mid", "Treble"]);
    }

    #[test]
    fn no_anc_and_ear_hook_shape() {
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
        controller.start();
        let mut sent = Vec::new();
        for (cmd, payload) in [
            (command::EQ_GET, vec![CUSTOM_CODE, 0]),
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
        controller.start();
        controller
            .feed(reply(direction::REPLY, command::EQ_GET, &[0, 0]))
            .unwrap();
        controller.take_outbound();
        let mut rejected = reply(direction::REPLY, command::LATENCY_GET, &[]);
        rejected[1] |= 0x03; // error code 3
        assert!(controller.feed(rejected).is_err());
        assert_eq!(controller.take_outbound()[3], command::DUAL_GET);
    }

    #[test]
    fn custom_curve_selects_custom_then_writes_it() {
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
        let controller = BarBudsController::new(DeviceModel::NothingEarOpen);
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
}
