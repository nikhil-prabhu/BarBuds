//! Google Pixel Buds Pro (1st gen), via Google's proprietary "Maestro" protocol.
//!
//! Maestro runs over an RFCOMM channel advertised under the service UUID
//! [`SERVICE_UUID`]. Every message is a pigweed `pw_rpc` packet ([`rpc`])
//! wrapped in an HDLC frame ([`hdlc`]), with protobuf payloads ([`proto`]).
//!
//! Connection sequence:
//! 1. The host doesn't know which RPC channel the earbuds will answer on, so
//!    [`PixelBudsProDriver::start`] sends `GetSoftwareInfo` on every candidate
//!    channel. The first successful reply fixes the channel.
//! 2. The driver then subscribes to runtime info (battery) and settings
//!    changes, and reads the current ANC mode and EQ curve one at a time.
//!
//! The earbuds have no EQ presets: they store one 5-band curve. The presets
//! offered here are the Pixel Buds app's own curves, which BarBuds writes; a
//! reported curve is shown as a preset when it matches one.
//!
//! Protocol knowledge comes from pbpctrl by Maximilian Luz
//! (<https://github.com/qzed/pbpctrl>, MIT/Apache-2.0).

#[cfg(test)]
mod fixtures;
mod hdlc;
mod proto;
mod rpc;
pub(crate) mod sim;

use std::collections::VecDeque;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::drivers::google::{self, HardwareInfo, SoftwareInfo, battery_status};
use crate::error::BarBudsError;
use crate::model::{
    AncMode, BudSide, ControlAction, ControlCapabilities, ControlGesture, ControlInput,
    ControlSetting, ControlSlot, DeviceCapabilities, EqCapabilities, EqPreset, EqSettings, EqUnit,
    FeatureToggle, FormFactor,
};
use prost::Message;
use proto::{Setting, action_target, anc_state, bool_setting, setting_id};

/// The on/off features shown, in display order, with their Maestro setting
/// IDs from pbpctrl. Volume EQ and in-ear detection are verified on hardware.
const TOGGLES: [(FeatureToggle, i32); 6] = [
    (
        FeatureToggle::ConversationDetection,
        bool_setting::SPEECH_DETECTION,
    ),
    (FeatureToggle::VolumeEq, bool_setting::VOLUME_EQ_ENABLE),
    (FeatureToggle::Multipoint, bool_setting::MULTIPOINT_ENABLE),
    (FeatureToggle::InEarDetection, bool_setting::OHD_ENABLE),
    (FeatureToggle::TouchControls, bool_setting::GESTURE_ENABLE),
    (FeatureToggle::MonoAudio, bool_setting::SUM_TO_MONO),
];
use rpc::{Method, RpcPacket, packet_type};

/// RFCOMM service UUID the earbuds advertise for Maestro.
pub(crate) const SERVICE_UUID: &str = "25e97ff7-24ce-4c4c-8951-f764a708f7b5";

const BAND_LABELS: [&str; 5] = ["Low Bass", "Bass", "Mid", "Treble", "Upper Treble"];
const MIN_GAIN_DB: f32 = -6.0;
const MAX_GAIN_DB: f32 = 6.0;
/// Two curves closer than this per band count as the same preset.
const GAIN_TOLERANCE_DB: f32 = 0.05;

/// pbpctrl uses this call ID for the channel probes and 0 for everything else.
const PROBE_CALL_ID: u32 = u32::MAX;
const CALL_ID: u32 = 0;

/// The Pixel Buds app's presets: `(id, name, curve)`. Each curve was read
/// from real Pixel Buds Pro after selecting the preset in the app
/// (2026-11-25). The app recognises a preset only by its exact curve, so
/// these must not be rounded: BudsLink's table rounds Light Bass and Balanced,
/// and writing those makes the app show "Last Saved" instead.
const PRESETS: [(&str, &str, [f32; 5]); 6] = [
    ("default", "Default", [0.0, 0.0, 0.0, 0.0, 0.0]),
    ("heavy-bass", "Heavy Bass", [5.0, 3.0, 0.0, 0.0, 0.0]),
    ("light-bass", "Light Bass", [-5.0, -1.5, 0.0, 0.0, 0.0]),
    ("balanced", "Balanced", [-3.5, 0.5, 1.0, -1.0, 2.5]),
    ("vocal-boost", "Vocal Boost", [-1.0, 0.0, 4.0, 2.0, 0.0]),
    ("clarity", "Clarity", [-2.0, 0.0, 2.0, 3.0, 5.0]),
];

/// The user's saved curve, offered as a preset like in the Pixel Buds app.
const LAST_SAVED_ID: &str = "last-saved";

fn preset_curve(id: &str) -> Option<[f32; 5]> {
    PRESETS
        .iter()
        .find(|&&(preset_id, _, _)| preset_id == id)
        .map(|&(_, _, curve)| curve)
}

fn same_curve(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() < GAIN_TOLERANCE_DB)
}

#[derive(Default)]
pub(crate) struct PixelBudsProDriver {
    /// The RPC channel the earbuds answered on. `None` until the handshake finishes.
    channel: Option<u32>,
    /// Settings still to read after the handshake. Sent one at a time, since
    /// concurrent calls to the same method would share one call ID.
    pending_reads: VecDeque<i32>,
    /// What each `WriteSetting` in flight will confirm, oldest first.
    pending_writes: VecDeque<Event>,
    /// The last EQ BarBuds wrote, so a curve it chose as "Custom" isn't shown
    /// as a matching preset when the earbuds echo it back.
    last_eq: Option<EqSettings>,
    /// The setting the in-flight `ReadSetting` asked for.
    reading: Option<i32>,
    /// The curve the earbuds currently play, as last reported.
    current_eq: Option<[f32; 5]>,
    /// The user's saved custom curve ("Last Saved"), once reported.
    last_saved: Option<[f32; 5]>,
    /// Each bud's touch & hold [`action_target`], left and right, once
    /// reported. Writes carry both, so changing one keeps the other.
    hold_targets: Option<(i32, i32)>,
    /// The noise-control cycle as last reported, so a write keeps the
    /// Adaptive flag on buds without Adaptive.
    anc_loop: Option<proto::AncrGestureLoop>,
    /// The Bluetooth name is the Pixel Buds Pro 2's factory name. Kept
    /// across `start()`.
    named_pro_2: bool,
    /// The buds reported Adaptive (as the mode or in the noise-control
    /// cycle), so they have it whatever their name.
    adaptive_seen: bool,
}

/// The noise-control modes, in the popover's order; Adaptive only on buds
/// that have it (see [`PixelBudsProDriver::has_adaptive`]).
const ANC_MODES: [AncMode; 4] = [
    AncMode::Off,
    AncMode::NoiseCancelling,
    AncMode::Transparency,
    AncMode::Adaptive,
];

fn anc_cycle(anc_loop: &proto::AncrGestureLoop) -> Vec<AncMode> {
    ANC_MODES
        .into_iter()
        .filter(|mode| match mode {
            AncMode::Off => anc_loop.off,
            AncMode::NoiseCancelling => anc_loop.active,
            AncMode::Transparency => anc_loop.aware,
            AncMode::Adaptive => anc_loop.adaptive,
        })
        .collect()
}

/// The touch & hold actions the Pixel Buds app offers (and pbpctrl sets).
const HOLD_ACTIONS: [(ControlAction, i32); 2] = [
    (ControlAction::NoiseControl, action_target::ANC_CONTROL),
    (
        ControlAction::VoiceAssistant,
        action_target::ASSISTANT_QUERY,
    ),
];

/// The action for an [`action_target`] value, including ones only other
/// firmware or apps set.
fn hold_action(target: i32) -> Option<ControlAction> {
    Some(match target {
        action_target::ANC_CONTROL => ControlAction::NoiseControl,
        action_target::ASSISTANT_QUERY => ControlAction::VoiceAssistant,
        action_target::PLAY_PAUSE => ControlAction::PlayPause,
        action_target::NEXT_TRACK => ControlAction::SkipForward,
        action_target::PREVIOUS_TRACK => ControlAction::SkipBack,
        _ => return None, // Unknown, or "check notifications".
    })
}

fn hold_settings((left, right): (i32, i32)) -> Vec<ControlSetting> {
    [(BudSide::Left, left), (BudSide::Right, right)]
        .into_iter()
        .filter_map(|(side, target)| {
            Some(ControlSetting {
                side,
                gesture: ControlGesture::Hold,
                action: hold_action(target)?,
            })
        })
        .collect()
}

impl PixelBudsProDriver {
    /// Pixel Buds Pro 2 (and any buds that report Adaptive) have the
    /// Adaptive mode; the first generation doesn't.
    fn has_adaptive(&self) -> bool {
        self.named_pro_2 || self.adaptive_seen
    }

    fn anc_modes(&self) -> Vec<AncMode> {
        ANC_MODES
            .into_iter()
            .filter(|&mode| mode != AncMode::Adaptive || self.has_adaptive())
            .collect()
    }

    fn channel(&self) -> Result<u32, BarBudsError> {
        self.channel
            .ok_or_else(|| BarBudsError::device("still connecting to the earbuds"))
    }

    fn request(&self, channel: u32, method: Method, payload: Vec<u8>) -> Vec<u8> {
        rpc::encode_request(channel, method, CALL_ID, payload)
    }

    fn send_next_read(&mut self, channel: u32, outbound: &mut Vec<u8>) {
        self.reading = self.pending_reads.pop_front();
        if let Some(id) = self.reading {
            outbound.extend(self.request(channel, Method::ReadSetting, proto::read_setting(id)));
        }
    }

    fn handle(
        &mut self,
        packet: RpcPacket,
        outbound: &mut Vec<u8>,
    ) -> Result<Option<Event>, BarBudsError> {
        let Some(method) = Method::from_packet(&packet) else {
            return Ok(None); // Another service; nothing we asked for.
        };

        let channel = match self.channel {
            Some(channel) => channel,
            None => {
                // Probe replies: the first success picks the channel. Errors from
                // the other candidates are expected and ignored.
                let ok = packet.packet_type == packet_type::RESPONSE && packet.status == 0;
                if method == Method::GetSoftwareInfo && ok {
                    let channel = packet.channel_id;
                    self.channel = Some(channel);
                    outbound.extend(self.request(channel, Method::GetHardwareInfo, vec![]));
                    outbound.extend(self.request(channel, Method::SubscribeRuntimeInfo, vec![]));
                    outbound.extend(self.request(
                        channel,
                        Method::SubscribeToSettingsChanges,
                        vec![],
                    ));
                    self.pending_reads = [
                        setting_id::CURRENT_ANCR_STATE,
                        setting_id::CURRENT_USER_EQ,
                        setting_id::LAST_SAVED_USER_EQ,
                        setting_id::VOLUME_ASYMMETRY,
                        setting_id::GESTURE_CONTROL,
                        setting_id::ANCR_GESTURE_LOOP,
                    ]
                    .into();
                    self.pending_reads.extend(TOGGLES.iter().map(|&(_, id)| id));
                    self.send_next_read(channel, outbound);
                    // The probe reply is the software info: the firmware versions.
                    let info = decode::<SoftwareInfo>(&packet.payload)?;
                    let firmware = google::firmware(&info, google::version_string_or_build);
                    return Ok(firmware.map(Event::Firmware));
                }
                return Ok(None);
            }
        };
        if packet.channel_id != channel {
            return Ok(None); // A late reply to a probe on another channel.
        }

        let failed = |what: &str| {
            BarBudsError::device(format!(
                "{what} failed: {}",
                rpc::status_name(packet.status)
            ))
        };
        let is_reply = matches!(
            packet.packet_type,
            packet_type::RESPONSE | packet_type::SERVER_ERROR
        );
        let ok = packet.packet_type != packet_type::SERVER_ERROR && packet.status == 0;

        match method {
            Method::SubscribeRuntimeInfo if packet.packet_type == packet_type::SERVER_STREAM => {
                let info = decode::<proto::RuntimeInfo>(&packet.payload)?;
                Ok(info
                    .battery_info
                    .map(|b| Event::Battery(battery_status(&b))))
            }
            Method::SubscribeToSettingsChanges
                if packet.packet_type == packet_type::SERVER_STREAM =>
            {
                self.setting_event(decode(&packet.payload)?)
            }
            Method::GetHardwareInfo if is_reply => {
                if !ok {
                    return Err(failed("reading serial numbers"));
                }
                let info = decode::<HardwareInfo>(&packet.payload)?;
                Ok(google::serial_numbers(&info).map(Event::SerialNumbers))
            }
            Method::ReadSetting if is_reply => {
                let id = self.reading.take();
                self.send_next_read(channel, outbound);
                if !ok {
                    // Some firmware refuses to share the saved curve; then
                    // "Last Saved" just isn't available.
                    if id == Some(setting_id::LAST_SAVED_USER_EQ) {
                        return Ok(None);
                    }
                    return Err(failed("reading a setting"));
                }
                self.setting_event(decode(&packet.payload)?)
            }
            Method::WriteSetting if is_reply => {
                let confirmed = self.pending_writes.pop_front();
                if !ok {
                    return Err(failed("changing a setting"));
                }
                if let (Some(Event::Controls(settings)), Some(targets)) =
                    (&confirmed, &mut self.hold_targets)
                {
                    for setting in settings {
                        let target = HOLD_ACTIONS
                            .iter()
                            .find(|&&(a, _)| a == setting.action)
                            .map(|&(_, t)| t);
                        match (setting.side, target) {
                            (BudSide::Left, Some(t)) => targets.0 = t,
                            (BudSide::Right, Some(t)) => targets.1 = t,
                            _ => {}
                        }
                    }
                }
                Ok(confirmed)
            }
            _ if packet.packet_type == packet_type::SERVER_ERROR => {
                Err(failed(&format!("{method:?}")))
            }
            _ => Ok(None),
        }
    }

    fn setting_event(&mut self, rsp: proto::SettingsRsp) -> Result<Option<Event>, BarBudsError> {
        match rsp.value.and_then(|v| v.value) {
            Some(Setting::CurrentAncrState(state)) => {
                if state == anc_state::ADAPTIVE {
                    self.adaptive_seen = true;
                }
                anc_mode(state).map(|mode| Some(Event::Anc(mode)))
            }
            Some(Setting::CurrentUserEq(bands)) => {
                self.current_eq = Some(bands.gains());
                Ok(Some(Event::Eq(self.eq_settings(bands.gains()))))
            }
            Some(Setting::VolumeAsymmetry(raw)) => {
                Ok(Some(Event::Balance(proto::balance_from_wire(raw))))
            }
            Some(Setting::AncrGestureLoop(anc_loop)) => {
                self.anc_loop = Some(anc_loop);
                if anc_loop.adaptive {
                    self.adaptive_seen = true;
                }
                Ok(Some(Event::AncCycle(anc_cycle(&anc_loop))))
            }
            Some(Setting::GestureControl(control)) => {
                let targets = control.targets();
                self.hold_targets = Some(targets);
                let settings = hold_settings(targets);
                Ok((!settings.is_empty()).then_some(Event::Controls(settings)))
            }
            Some(Setting::LastSavedUserEq(bands)) => {
                self.last_saved = Some(bands.gains());
                // The current curve may now be recognisable as "Last Saved".
                Ok(self
                    .current_eq
                    .map(|current| Event::Eq(self.eq_settings(current))))
            }
            Some(setting) => {
                let (id, on) = setting.as_bool().expect("other settings are matched above");
                let toggle = TOGGLES
                    .iter()
                    .find(|&&(_, t)| t == id)
                    .map(|&(toggle, _)| toggle);
                Ok(toggle.map(|toggle| Event::Toggle(toggle, on)))
            }
            None => Ok(None), // A setting BarBuds doesn't show.
        }
    }

    fn eq_settings(&self, gains: [f32; 5]) -> EqSettings {
        if let Some(last) = &self.last_eq
            && same_curve(&last.gains, &gains)
        {
            return EqSettings {
                preset: last.preset.clone(),
                gains: gains.to_vec(),
            };
        }
        // Like the Pixel Buds app: the app's presets first, then the user's
        // saved curve, else a custom curve.
        let preset = PRESETS
            .iter()
            .find(|(_, _, curve)| same_curve(curve, &gains))
            .map(|&(id, _, _)| id.to_string())
            .or_else(|| {
                self.last_saved
                    .filter(|saved| same_curve(saved, &gains))
                    .map(|_| LAST_SAVED_ID.to_string())
            });
        EqSettings {
            preset,
            gains: gains.to_vec(),
        }
    }

    fn write(&mut self, setting: Setting, confirms: Event) -> Result<Vec<u8>, BarBudsError> {
        let channel = self.channel()?;
        self.pending_writes.push_back(confirms);
        Ok(self.request(channel, Method::WriteSetting, proto::write_setting(setting)))
    }
}

fn decode<M: Message + Default>(payload: &[u8]) -> Result<M, BarBudsError> {
    M::decode(payload).map_err(|err| BarBudsError::malformed(format!("bad Maestro message: {err}")))
}

fn anc_mode(state: i32) -> Result<Option<AncMode>, BarBudsError> {
    match state {
        anc_state::UNKNOWN => Ok(None),
        anc_state::OFF => Ok(Some(AncMode::Off)),
        anc_state::ACTIVE => Ok(Some(AncMode::NoiseCancelling)),
        anc_state::AWARE => Ok(Some(AncMode::Transparency)),
        anc_state::ADAPTIVE => Ok(Some(AncMode::Adaptive)),
        other => Err(BarBudsError::malformed(format!(
            "unsupported ANC state {other}"
        ))),
    }
}

fn anc_state(mode: AncMode) -> i32 {
    match mode {
        AncMode::Off => anc_state::OFF,
        AncMode::NoiseCancelling => anc_state::ACTIVE,
        AncMode::Transparency => anc_state::AWARE,
        AncMode::Adaptive => anc_state::ADAPTIVE,
    }
}

impl EarbudDriver for PixelBudsProDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            display_name: if self.named_pro_2 {
                "Pixel Buds Pro 2"
            } else {
                "Pixel Buds Pro"
            }
            .to_string(),
            anc_modes: self.anc_modes(),
            eq: Some(EqCapabilities {
                band_labels: BAND_LABELS.map(String::from).to_vec(),
                unit: EqUnit::Decibels,
                min_gain: MIN_GAIN_DB,
                max_gain: MAX_GAIN_DB,
                step: 0.5,
                presets: PRESETS
                    .iter()
                    .map(|&(id, name, _)| (id, name))
                    .chain([(LAST_SAVED_ID, "Last Saved")])
                    .map(|(id, name)| EqPreset {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                    .collect(),
            }),
            has_charging_case: true,
            form_factor: FormFactor::Stemless,
            toggles: TOGGLES.iter().map(|&(toggle, _)| toggle).collect(),
            can_ring: true,
            has_balance: true,
            anc_cycle_modes: self.anc_modes(),
            spatial_audio_modes: vec![],
            anc_strengths: vec![],
            controls: Some(ControlCapabilities {
                // The Pixel Buds app says "Press & hold".
                input: ControlInput::Press,
                slots: [BudSide::Left, BudSide::Right]
                    .into_iter()
                    .map(|side| ControlSlot {
                        side,
                        gesture: ControlGesture::Hold,
                        actions: HOLD_ACTIONS.iter().map(|&(a, _)| a).collect(),
                    })
                    .collect(),
            }),
        }
    }

    fn set_device_name(&mut self, name: &str) {
        self.named_pro_2 = name.trim().to_lowercase().ends_with("pixel buds pro 2");
    }

    fn start(&mut self) -> Vec<u8> {
        *self = Self {
            named_pro_2: self.named_pro_2,
            ..Self::default()
        };
        rpc::CANDIDATE_CHANNELS
            .into_iter()
            .flat_map(|channel| {
                rpc::encode_request(channel, Method::GetSoftwareInfo, PROBE_CALL_ID, vec![])
            })
            .collect()
    }

    fn parse_frame(&mut self, buf: &[u8], outbound: &mut Vec<u8>) -> Option<Decoded> {
        let (consumed, frame) = hdlc::decode(buf)?;
        let result = match frame {
            Ok(Some(frame)) => {
                rpc::decode_packet(&frame).and_then(|packet| self.handle(packet, outbound))
            }
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        };
        Some(Decoded { consumed, result })
    }

    fn encode_anc(&mut self, mode: AncMode) -> Result<Vec<u8>, BarBudsError> {
        self.write(
            Setting::CurrentAncrState(anc_state(mode)),
            Event::Anc(Some(mode)),
        )
    }

    fn encode_toggle(&mut self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        let setting = TOGGLES
            .iter()
            .find(|&&(t, _)| t == toggle)
            .and_then(|&(_, id)| Setting::from_bool(id, on))
            .ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("{toggle:?}"),
            })?;
        self.write(setting, Event::Toggle(toggle, on))
    }

    fn encode_control(&mut self, setting: ControlSetting) -> Result<Vec<u8>, BarBudsError> {
        let (left, right) = self.hold_targets.ok_or_else(|| {
            BarBudsError::device("the earbuds haven't reported their gestures yet")
        })?;
        let target = HOLD_ACTIONS
            .iter()
            .find(|&&(a, _)| a == setting.action)
            .map(|&(_, t)| t)
            .ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("{setting:?}"),
            })?;
        let control = match setting.side {
            BudSide::Left => proto::GestureControl::new(target, right),
            BudSide::Right => proto::GestureControl::new(left, target),
        };
        self.write(
            Setting::GestureControl(control),
            Event::Controls(vec![setting]),
        )
    }

    fn encode_anc_cycle(&mut self, modes: &[AncMode]) -> Result<Vec<u8>, BarBudsError> {
        // Buds without Adaptive get their own flag back unchanged.
        let adaptive = if self.has_adaptive() {
            modes.contains(&AncMode::Adaptive)
        } else {
            self.anc_loop.is_some_and(|l| l.adaptive)
        };
        let anc_loop = proto::AncrGestureLoop {
            active: modes.contains(&AncMode::NoiseCancelling),
            off: modes.contains(&AncMode::Off),
            aware: modes.contains(&AncMode::Transparency),
            adaptive,
        };
        self.write(
            Setting::AncrGestureLoop(anc_loop),
            Event::AncCycle(modes.to_vec()),
        )
    }

    fn encode_balance(&mut self, balance: i32) -> Result<Vec<u8>, BarBudsError> {
        self.write(
            Setting::VolumeAsymmetry(proto::balance_to_wire(balance)),
            Event::Balance(balance),
        )
    }

    fn encode_eq(&mut self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let gains = match &eq.preset {
            Some(id) if id == LAST_SAVED_ID => self.last_saved.ok_or_else(|| {
                BarBudsError::device("these earbuds haven't shared a saved curve")
            })?,
            Some(id) => preset_curve(id).ok_or_else(|| BarBudsError::Unsupported {
                feature: format!("EQ preset {id:?}"),
            })?,
            None => custom_gains(&eq.gains)?,
        };
        let settings = EqSettings {
            preset: eq.preset.clone(),
            gains: gains.to_vec(),
        };
        let frame = self.write(
            Setting::CurrentUserEq(proto::EqBands::from_gains(gains)),
            Event::Eq(settings.clone()),
        )?;
        self.last_eq = Some(settings);
        Ok(frame)
    }
}

fn custom_gains(gains: &[f32]) -> Result<[f32; 5], BarBudsError> {
    let gains: [f32; 5] = gains.try_into().map_err(|_| {
        BarBudsError::invalid_argument(format!("expected 5 EQ bands, got {}", gains.len()))
    })?;
    if let Some(label) = gains
        .iter()
        .zip(BAND_LABELS)
        .find(|(g, _)| !g.is_finite())
        .map(|(_, l)| l)
    {
        return Err(BarBudsError::invalid_argument(format!(
            "EQ gain for {label} is not a finite number"
        )));
    }
    Ok(gains.map(|g| g.clamp(MIN_GAIN_DB, MAX_GAIN_DB)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::sim::PixelBudsSim;
    use super::*;
    use crate::controller::BarBudsController;
    use crate::model::{BatteryReading, DeviceModel, DeviceState};

    /// Deliver `bytes` to the simulated earbuds and keep relaying in both
    /// directions until neither side has anything more to send, like the
    /// Swift session does. Returns the first error `feed` reported.
    fn exchange(
        controller: &BarBudsController,
        sim: &mut PixelBudsSim,
        bytes: Vec<u8>,
    ) -> Option<BarBudsError> {
        let mut to_device = bytes;
        let mut first_error = None;
        while !to_device.is_empty() {
            let reply = sim.receive(&to_device);
            if let Err(err) = controller.feed(reply) {
                first_error.get_or_insert(err);
            }
            to_device = controller.take_outbound();
        }
        first_error
    }

    fn connected() -> (Arc<BarBudsController>, PixelBudsSim) {
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        let mut sim = PixelBudsSim::new(24);
        assert_eq!(exchange(&controller, &mut sim, controller.start()), None);
        (controller, sim)
    }

    #[test]
    fn handshake_finds_channel_and_reads_state() {
        let (controller, sim) = connected();
        assert_eq!(sim.subscriptions(), (true, true));
        let state = controller.state();
        assert_eq!(
            state.battery.left,
            Some(BatteryReading {
                percent: 71,
                charging: false
            })
        );
        assert_eq!(
            state.battery.right,
            Some(BatteryReading {
                percent: 69,
                charging: false
            })
        );
        assert_eq!(
            state.battery.charging_case,
            Some(BatteryReading {
                percent: 45,
                charging: true
            })
        );
        assert_eq!(state.anc, Some(AncMode::NoiseCancelling));
        assert_eq!(
            state.eq,
            Some(EqSettings {
                preset: None,
                gains: vec![1.0, 0.5, 0.0, -1.5, 2.0]
            })
        );
        assert!(state.toggles[&FeatureToggle::InEarDetection]);
        assert!(!state.toggles[&FeatureToggle::VolumeEq]);
    }

    #[test]
    fn handshake_reads_firmware_and_serial_numbers() {
        let (controller, _) = connected();
        let info = controller.state().info;
        let release = Some("release_5.11_signed".to_string());
        assert_eq!(info.firmware.left, release);
        assert_eq!(info.firmware.right, release);
        assert_eq!(info.firmware.charging_case, release);
        assert_eq!(info.serial_numbers.left.as_deref(), Some("SIMLEFT01"));
        assert_eq!(info.serial_numbers.right.as_deref(), Some("SIMRIGHT01"));
        assert_eq!(
            info.serial_numbers.charging_case.as_deref(),
            Some("SIMCASE01")
        );
    }

    #[test]
    fn reads_and_writes_every_toggle() {
        let (controller, mut sim) = connected();
        let caps = controller.capabilities();
        assert_eq!(caps.toggles.len(), 6);
        assert!(!caps.toggles.contains(&FeatureToggle::AdaptiveSound));
        let state = controller.state();
        for &toggle in &caps.toggles {
            let on = state.toggles[&toggle];
            let frame = controller.encode_toggle(toggle, !on).unwrap();
            assert_eq!(exchange(&controller, &mut sim, frame), None);
            assert_eq!(controller.state().toggles[&toggle], !on, "{toggle:?}");
        }
    }

    #[test]
    fn toggles_apply_after_device_confirms() {
        let (controller, mut sim) = connected();
        let frame = controller
            .encode_toggle(FeatureToggle::VolumeEq, true)
            .unwrap();
        assert!(!controller.state().toggles[&FeatureToggle::VolumeEq]);
        assert_eq!(exchange(&controller, &mut sim, frame), None);
        assert!(controller.state().toggles[&FeatureToggle::VolumeEq]);

        let frame = controller
            .encode_toggle(FeatureToggle::InEarDetection, false)
            .unwrap();
        assert_eq!(exchange(&controller, &mut sim, frame), None);
        assert!(!controller.state().toggles[&FeatureToggle::InEarDetection]);

        assert!(matches!(
            controller.encode_toggle(FeatureToggle::AdaptiveSound, true),
            Err(BarBudsError::Unsupported { .. })
        ));
    }

    #[test]
    fn handshake_survives_byte_at_a_time_delivery() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        let mut sim = PixelBudsSim::new(19);
        let mut to_device = controller.start();
        while !to_device.is_empty() {
            for byte in sim.receive(&to_device) {
                controller.feed(vec![byte]).unwrap();
            }
            to_device = controller.take_outbound();
        }
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        assert!(controller.state().eq.is_some());
    }

    #[test]
    fn settings_wait_for_handshake() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        assert!(matches!(
            controller.encode_anc(AncMode::Off),
            Err(BarBudsError::Device { .. })
        ));
    }

    #[test]
    fn anc_change_applies_after_device_confirms() {
        let (controller, mut sim) = connected();
        let frame = controller.encode_anc(AncMode::Transparency).unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
        assert_eq!(exchange(&controller, &mut sim, frame), None);
        assert_eq!(controller.state().anc, Some(AncMode::Transparency));
        assert_eq!(sim.anc(), anc_state::AWARE);
    }

    #[test]
    fn rejected_write_reports_error_and_keeps_state() {
        let (controller, mut sim) = connected();
        sim.reject_writes();
        let frame = controller.encode_anc(AncMode::Off).unwrap();
        let err = exchange(&controller, &mut sim, frame);
        assert!(matches!(err, Some(BarBudsError::Device { .. })), "{err:?}");
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
    }

    #[test]
    fn eq_presets_and_custom_curves() {
        let (controller, mut sim) = connected();

        let bass = EqSettings {
            preset: Some("heavy-bass".to_string()),
            gains: vec![],
        };
        exchange(&controller, &mut sim, controller.encode_eq(bass).unwrap());
        let eq = controller.state().eq.unwrap();
        assert_eq!(eq.preset.as_deref(), Some("heavy-bass"));
        assert_eq!(eq.gains, [5.0, 3.0, 0.0, 0.0, 0.0]);

        // Custom gains are clamped to the device range.
        let custom = EqSettings {
            preset: None,
            gains: vec![-9.0, 0.0, 1.5, 0.0, 9.0],
        };
        exchange(&controller, &mut sim, controller.encode_eq(custom).unwrap());
        assert_eq!(sim.eq(), [-6.0, 0.0, 1.5, 0.0, 6.0]);
        assert_eq!(controller.state().eq.unwrap().preset, None);

        // A custom curve that happens to match the Default preset stays custom.
        let flat_custom = EqSettings {
            preset: None,
            gains: vec![0.0; 5],
        };
        exchange(
            &controller,
            &mut sim,
            controller.encode_eq(flat_custom).unwrap(),
        );
        assert_eq!(controller.state().eq.unwrap().preset, None);

        // Unknown preset IDs are refused.
        let unknown = EqSettings {
            preset: Some("party".to_string()),
            gains: vec![],
        };
        assert!(matches!(
            controller.encode_eq(unknown),
            Err(BarBudsError::Unsupported { .. })
        ));
    }

    #[test]
    fn balance_reads_and_writes() {
        let (controller, mut sim) = connected();
        assert!(controller.capabilities().has_balance);
        assert_eq!(controller.state().balance, Some(0));

        let frame = controller.encode_balance(-30).unwrap();
        assert_eq!(controller.state().balance, Some(0), "waits for the device");
        assert_eq!(exchange(&controller, &mut sim, frame), None);
        assert_eq!(controller.state().balance, Some(-30));
        // 30 toward the left is stored as 60 (even = left side louder).
        assert_eq!(sim.volume_asymmetry(), 60);

        assert!(matches!(
            controller.encode_balance(101),
            Err(BarBudsError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn touch_and_hold_reads_and_writes_one_side() {
        use crate::model::{BudSide, ControlAction, ControlGesture, ControlSetting};
        let (controller, mut sim) = connected();
        let hold = |side, action| ControlSetting {
            side,
            gesture: ControlGesture::Hold,
            action,
        };
        let state = controller.state();
        assert!(
            state
                .controls
                .contains(&hold(BudSide::Left, ControlAction::NoiseControl))
        );
        assert!(
            state
                .controls
                .contains(&hold(BudSide::Right, ControlAction::NoiseControl))
        );

        let assistant = hold(BudSide::Left, ControlAction::VoiceAssistant);
        let frame = controller.encode_control(assistant).unwrap();
        assert!(
            !controller.state().controls.contains(&assistant),
            "waits for the device"
        );
        exchange(&controller, &mut sim, frame);
        assert!(controller.state().controls.contains(&assistant));
        // Both sides are written; the right one is kept.
        assert_eq!(
            sim.hold_targets(),
            (action_target::ASSISTANT_QUERY, action_target::ANC_CONTROL)
        );

        // Only the Pixel Buds app's two actions are offered.
        assert!(
            controller
                .encode_control(hold(BudSide::Right, ControlAction::PlayPause))
                .is_err()
        );
    }

    #[test]
    fn noise_control_cycle_reads_and_writes() {
        let (controller, mut sim) = connected();
        assert_eq!(
            controller.state().anc_cycle,
            Some(vec![AncMode::NoiseCancelling, AncMode::Transparency])
        );
        let all = vec![
            AncMode::Off,
            AncMode::NoiseCancelling,
            AncMode::Transparency,
        ];
        let frame = controller.encode_anc_cycle(all.clone()).unwrap();
        exchange(&controller, &mut sim, frame);
        assert_eq!(controller.state().anc_cycle, Some(all));
        // At least two modes, as the Pixel Buds app requires.
        assert!(matches!(
            controller.encode_anc_cycle(vec![AncMode::Off]),
            Err(BarBudsError::InvalidArgument { .. })
        ));
        // Order and duplicates don't matter.
        let frame = controller
            .encode_anc_cycle(vec![AncMode::Transparency, AncMode::Off, AncMode::Off])
            .unwrap();
        exchange(&controller, &mut sim, frame);
        assert_eq!(
            controller.state().anc_cycle,
            Some(vec![AncMode::Off, AncMode::Transparency])
        );
    }

    #[test]
    fn noise_control_cycle_keeps_adaptive() {
        // pbpctrl's layout: active = 1, off = 2, aware = 3, adaptive = 4.
        let mut driver = PixelBudsProDriver {
            channel: Some(21),
            anc_loop: Some(proto::AncrGestureLoop {
                active: true,
                off: false,
                aware: true,
                adaptive: true,
            }),
            ..Default::default()
        };
        driver
            .encode_anc_cycle(&[AncMode::Off, AncMode::NoiseCancelling])
            .unwrap();
        let Some(Event::AncCycle(modes)) = driver.pending_writes.back() else {
            panic!("no pending cycle write");
        };
        assert_eq!(modes, &[AncMode::Off, AncMode::NoiseCancelling]);
        let anc_loop = proto::AncrGestureLoop {
            active: true,
            off: true,
            aware: false,
            adaptive: true,
        };
        assert_eq!(
            anc_loop.encode_to_vec(),
            [0x08, 0x01, 0x10, 0x01, 0x20, 0x01]
        );
    }

    #[test]
    fn gesture_control_wire_format() {
        // pbpctrl's layout: left = 1, right = 2, each { type = 4 { value = 1 } }.
        let control =
            proto::GestureControl::new(action_target::ASSISTANT_QUERY, action_target::ANC_CONTROL);
        assert_eq!(
            control.encode_to_vec(),
            [
                0x0a, 0x04, 0x22, 0x02, 0x08, 0x06, 0x12, 0x04, 0x22, 0x02, 0x08, 0x05
            ]
        );
        // Actions BarBuds can't name (e.g. "check notifications") are skipped.
        let settings = hold_settings((1, action_target::ASSISTANT_QUERY));
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0].side, crate::model::BudSide::Right);
    }

    #[test]
    fn last_saved_curve_is_a_preset() {
        let (controller, mut sim) = connected();
        let caps = controller.capabilities().eq.unwrap();
        assert_eq!(caps.presets.last().unwrap().name, "Last Saved");

        // Selecting it writes the saved curve the earbuds reported.
        let request = EqSettings {
            preset: Some("last-saved".to_string()),
            gains: vec![],
        };
        exchange(
            &controller,
            &mut sim,
            controller.encode_eq(request).unwrap(),
        );
        assert_eq!(sim.eq(), [2.0, 1.0, 0.0, 1.0, 2.0]);
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("last-saved")
        );

        // The saved curve set from a phone is recognised too.
        controller
            .feed(sim.external_eq_change([2.0, 1.0, 0.0, 1.0, 2.0]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("last-saved")
        );
    }

    #[test]
    fn refused_last_saved_read_is_silent() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        let mut sim = PixelBudsSim::new(24);
        sim.refuse_last_saved();
        // No error surfaces, and the rest of the handshake still completes.
        assert_eq!(exchange(&controller, &mut sim, controller.start()), None);
        assert!(
            controller
                .state()
                .toggles
                .contains_key(&FeatureToggle::MonoAudio)
        );

        let request = EqSettings {
            preset: Some("last-saved".to_string()),
            gains: vec![],
        };
        assert!(matches!(
            controller.encode_eq(request),
            Err(BarBudsError::Device { .. })
        ));
    }

    #[test]
    fn custom_eq_rejects_wrong_band_count_and_nan() {
        let (controller, _) = connected();
        for gains in [vec![0.0; 3], vec![0.0, f32::NAN, 0.0, 0.0, 0.0]] {
            let eq = EqSettings {
                preset: None,
                gains,
            };
            assert!(matches!(
                controller.encode_eq(eq),
                Err(BarBudsError::InvalidArgument { .. })
            ));
        }
    }

    #[test]
    fn changes_made_elsewhere_are_pushed() {
        // E.g. the user long-presses a bud, or changes ANC from their phone.
        let (controller, mut sim) = connected();
        controller
            .feed(sim.external_anc_change(anc_state::OFF))
            .unwrap();
        assert_eq!(controller.state().anc, Some(AncMode::Off));

        controller
            .feed(sim.external_eq_change([-2.0, 0.0, 2.0, 3.0, 5.0]))
            .unwrap();
        assert_eq!(
            controller.state().eq.unwrap().preset.as_deref(),
            Some("clarity")
        );

        controller.feed(sim.battery_update(70, None)).unwrap();
        let battery = controller.state().battery;
        assert_eq!(battery.left.unwrap().percent, 70);
        assert_eq!(battery.charging_case, None);
    }

    #[test]
    fn unsupported_anc_state_is_an_error_but_state_survives() {
        let (controller, mut sim) = connected();
        let err = controller.feed(sim.external_anc_change(5)).unwrap_err(); // no such state
        assert!(matches!(err, BarBudsError::MalformedFrame { .. }));
        assert_eq!(controller.state().anc, Some(AncMode::NoiseCancelling));
    }

    #[test]
    fn reset_forgets_the_channel() {
        let (controller, _) = connected();
        controller.reset();
        assert_eq!(controller.state(), DeviceState::default());
        assert!(controller.encode_anc(AncMode::Off).is_err());
    }

    #[test]
    fn the_first_generation_has_no_adaptive() {
        let (controller, _) = connected();
        let caps = controller.capabilities();
        assert_eq!(caps.display_name, "Pixel Buds Pro");
        assert!(!caps.anc_modes.contains(&AncMode::Adaptive));
        assert!(!caps.anc_cycle_modes.contains(&AncMode::Adaptive));
        assert!(controller.encode_anc(AncMode::Adaptive).is_err());
    }

    #[test]
    fn pixel_buds_pro_2_by_name_have_adaptive() {
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        controller.set_device_name("Pixel Buds Pro 2".to_string());
        let mut sim = PixelBudsSim::new(24);
        exchange(&controller, &mut sim, controller.start());
        let caps = controller.capabilities();
        assert_eq!(caps.display_name, "Pixel Buds Pro 2");
        assert_eq!(
            caps.anc_modes,
            [
                AncMode::Off,
                AncMode::NoiseCancelling,
                AncMode::Transparency,
                AncMode::Adaptive
            ]
        );
        // Adaptive is Maestro state 4.
        exchange(
            &controller,
            &mut sim,
            controller.encode_anc(AncMode::Adaptive).unwrap(),
        );
        assert_eq!(controller.state().anc, Some(AncMode::Adaptive));
        assert_eq!(sim.anc(), anc_state::ADAPTIVE);
        // And it can join the press & hold cycle.
        let cycle = vec![AncMode::NoiseCancelling, AncMode::Adaptive];
        exchange(
            &controller,
            &mut sim,
            controller.encode_anc_cycle(cycle.clone()).unwrap(),
        );
        assert_eq!(controller.state().anc_cycle, Some(cycle));
    }

    #[test]
    fn renamed_buds_that_report_adaptive_get_it() {
        // A renamed Pro 2: Adaptive shows up from what the buds report.
        let controller = BarBudsController::new(DeviceModel::PixelBudsPro);
        controller.set_device_name("Nikhil's buds".to_string());
        let mut sim = PixelBudsSim::new(24).with_adaptive();
        exchange(&controller, &mut sim, controller.start());
        assert_eq!(controller.state().anc, Some(AncMode::Adaptive));
        let caps = controller.capabilities();
        assert_eq!(caps.display_name, "Pixel Buds Pro");
        assert!(caps.anc_modes.contains(&AncMode::Adaptive));
        assert!(caps.anc_cycle_modes.contains(&AncMode::Adaptive));
        assert!(
            controller
                .state()
                .anc_cycle
                .unwrap()
                .contains(&AncMode::Adaptive)
        );
    }
}
