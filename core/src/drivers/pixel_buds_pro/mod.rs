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
//! offered here are curves BarBuds writes, and a reported curve is shown as a
//! preset when it matches one.
//!
//! Protocol knowledge comes from pbpctrl by Maximilian Luz
//! (<https://github.com/qzed/pbpctrl>, MIT/Apache-2.0).

#[cfg(test)]
mod fixtures;
mod hdlc;
mod proto;
mod rpc;
#[cfg(test)]
mod sim;

use std::collections::VecDeque;

use crate::driver::{Decoded, EarbudDriver, Event};
use crate::drivers::google::{self, HardwareInfo, SoftwareInfo, battery_status};
use crate::error::BarBudsError;
use crate::model::{
    AncMode, DeviceCapabilities, EqCapabilities, EqPreset, EqSettings, EqUnit, FeatureToggle,
};
use prost::Message;
use proto::{Setting, anc_state, bool_setting, setting_id};

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

fn preset_curve(preset: EqPreset) -> Option<[f32; 5]> {
    match preset {
        EqPreset::Flat => Some([0.0, 0.0, 0.0, 0.0, 0.0]),
        EqPreset::BassBoost => Some([5.0, 3.0, 0.0, 0.0, 0.0]),
        EqPreset::Vocal => Some([-2.0, 0.0, 3.0, 2.0, 0.0]),
        EqPreset::TrebleBoost => Some([0.0, 0.0, 0.0, 3.0, 5.0]),
        EqPreset::Custom => None,
    }
}

const PRESETS: [EqPreset; 5] = [
    EqPreset::Flat,
    EqPreset::BassBoost,
    EqPreset::Vocal,
    EqPreset::TrebleBoost,
    EqPreset::Custom,
];

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
}

impl PixelBudsProDriver {
    fn channel(&self) -> Result<u32, BarBudsError> {
        self.channel
            .ok_or_else(|| BarBudsError::device("still connecting to the earbuds"))
    }

    fn request(&self, channel: u32, method: Method, payload: Vec<u8>) -> Vec<u8> {
        rpc::encode_request(channel, method, CALL_ID, payload)
    }

    fn send_next_read(&mut self, channel: u32, outbound: &mut Vec<u8>) {
        if let Some(id) = self.pending_reads.pop_front() {
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
                    self.pending_reads =
                        [setting_id::CURRENT_ANCR_STATE, setting_id::CURRENT_USER_EQ].into();
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
                self.send_next_read(channel, outbound);
                if !ok {
                    return Err(failed("reading a setting"));
                }
                self.setting_event(decode(&packet.payload)?)
            }
            Method::WriteSetting if is_reply => {
                let confirmed = self.pending_writes.pop_front();
                if !ok {
                    return Err(failed("changing a setting"));
                }
                Ok(confirmed)
            }
            _ if packet.packet_type == packet_type::SERVER_ERROR => {
                Err(failed(&format!("{method:?}")))
            }
            _ => Ok(None),
        }
    }

    fn setting_event(&self, rsp: proto::SettingsRsp) -> Result<Option<Event>, BarBudsError> {
        match rsp.value.and_then(|v| v.value) {
            Some(Setting::CurrentAncrState(state)) => {
                anc_mode(state).map(|mode| Some(Event::Anc(mode)))
            }
            Some(Setting::CurrentUserEq(bands)) => {
                Ok(Some(Event::Eq(self.eq_settings(bands.gains()))))
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
                preset: last.preset,
                gains: gains.to_vec(),
            };
        }
        let preset = PRESETS
            .into_iter()
            .find(|&p| preset_curve(p).is_some_and(|curve| same_curve(&curve, &gains)))
            .unwrap_or(EqPreset::Custom);
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
    }
}

impl EarbudDriver for PixelBudsProDriver {
    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            display_name: "Pixel Buds Pro".to_string(),
            anc_modes: vec![
                AncMode::Off,
                AncMode::NoiseCancelling,
                AncMode::Transparency,
            ],
            eq: Some(EqCapabilities {
                band_labels: BAND_LABELS.map(String::from).to_vec(),
                unit: EqUnit::Decibels,
                min_gain: MIN_GAIN_DB,
                max_gain: MAX_GAIN_DB,
                step: 0.5,
                presets: PRESETS.to_vec(),
            }),
            has_charging_case: true,
            toggles: TOGGLES.iter().map(|&(toggle, _)| toggle).collect(),
        }
    }

    fn start(&mut self) -> Vec<u8> {
        *self = Self::default();
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

    fn encode_eq(&mut self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError> {
        let gains = match preset_curve(eq.preset) {
            Some(curve) => curve,
            None => custom_gains(&eq.gains)?,
        };
        let settings = EqSettings {
            preset: eq.preset,
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
                preset: EqPreset::Custom,
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
            preset: EqPreset::BassBoost,
            gains: vec![],
        };
        exchange(&controller, &mut sim, controller.encode_eq(bass).unwrap());
        let eq = controller.state().eq.unwrap();
        assert_eq!(eq.preset, EqPreset::BassBoost);
        assert_eq!(eq.gains, [5.0, 3.0, 0.0, 0.0, 0.0]);

        // Custom gains are clamped to the device range.
        let custom = EqSettings {
            preset: EqPreset::Custom,
            gains: vec![-9.0, 0.0, 1.5, 0.0, 9.0],
        };
        exchange(&controller, &mut sim, controller.encode_eq(custom).unwrap());
        assert_eq!(sim.eq(), [-6.0, 0.0, 1.5, 0.0, 6.0]);
        assert_eq!(controller.state().eq.unwrap().preset, EqPreset::Custom);

        // A Custom curve that happens to be flat stays Custom.
        let flat_custom = EqSettings {
            preset: EqPreset::Custom,
            gains: vec![0.0; 5],
        };
        exchange(
            &controller,
            &mut sim,
            controller.encode_eq(flat_custom).unwrap(),
        );
        assert_eq!(controller.state().eq.unwrap().preset, EqPreset::Custom);
    }

    #[test]
    fn custom_eq_rejects_wrong_band_count_and_nan() {
        let (controller, _) = connected();
        for gains in [vec![0.0; 3], vec![0.0, f32::NAN, 0.0, 0.0, 0.0]] {
            let eq = EqSettings {
                preset: EqPreset::Custom,
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
            .feed(sim.external_eq_change([0.0, 0.0, 0.0, 3.0, 5.0]))
            .unwrap();
        assert_eq!(controller.state().eq.unwrap().preset, EqPreset::TrebleBoost);

        controller.feed(sim.battery_update(70, None)).unwrap();
        let battery = controller.state().battery;
        assert_eq!(battery.left.unwrap().percent, 70);
        assert_eq!(battery.charging_case, None);
    }

    #[test]
    fn unsupported_anc_state_is_an_error_but_state_survives() {
        let (controller, mut sim) = connected();
        let err = controller.feed(sim.external_anc_change(4)).unwrap_err(); // "adaptive"
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
}
