//! A simulated pair of Pixel Buds Pro: enough of the Maestro server side to
//! exercise the driver end to end in tests, and to stand in for real earbuds
//! in the app's Debug menu (see [`crate::DeviceSimulator`]).

use std::collections::HashMap;

use crate::drivers::google::{
    FirmwareInfo, FirmwareVersion, HardwareInfo, SerialNumbers, SoftwareInfo,
};

use prost::Message;

use super::proto::{
    self, Setting, SettingValue, SettingsRsp, anc_state, battery_state, bool_setting, setting_id,
};
use super::rpc::{self, Method, RpcPacket, packet_type};
use super::{hdlc, rpc::MAESTRO_SERVICE};

/// What pbpctrl saw when reading the saved curve failed.
#[cfg(test)]
const STATUS_UNKNOWN: u32 = 2;
const STATUS_INVALID_ARGUMENT: u32 = 3;
/// Sent for requests on channels the device doesn't serve, and unknown settings.
const STATUS_NOT_FOUND: u32 = 5;
#[cfg(test)]
const STATUS_FAILED_PRECONDITION: u32 = 9;
const STATUS_UNIMPLEMENTED: u32 = 12;

/// The RPC channel the simulated earbuds answer on (the right bud's).
pub(crate) const DEFAULT_CHANNEL: u32 = 21;

pub(crate) struct PixelBudsSim {
    channel: u32,
    rx: Vec<u8>,
    anc: i32,
    eq: proto::EqBands,
    /// The user's saved custom curve ("Last Saved").
    last_saved: proto::EqBands,
    /// Raw `volume_asymmetry` (balance) value.
    volume_asymmetry: i32,
    /// Touch & hold actions; both buds cycle noise control by default.
    gesture_control: proto::GestureControl,
    /// Noise-control cycle; ANC and Transparency by default.
    anc_loop: proto::AncrGestureLoop,
    /// Refuse to share the saved curve, as pbpctrl saw on some firmware.
    #[cfg(test)]
    refuse_last_saved: bool,
    /// On/off settings by ID.
    bools: HashMap<i32, bool>,
    battery: proto::BatteryInfo,
    runtime_subscribed: bool,
    settings_subscribed: bool,
    #[cfg(test)]
    reject_writes: bool,
}

fn battery(level: i32, charging: bool) -> Option<proto::DeviceBatteryInfo> {
    let state = if charging {
        battery_state::CHARGING
    } else {
        battery_state::NOT_CHARGING
    };
    Some(proto::DeviceBatteryInfo { level, state })
}

impl PixelBudsSim {
    /// Earbuds that serve Maestro on `channel` only.
    pub fn new(channel: u32) -> Self {
        Self {
            channel,
            rx: Vec::new(),
            anc: anc_state::ACTIVE,
            bools: HashMap::from([
                (bool_setting::OHD_ENABLE, true),
                (bool_setting::GESTURE_ENABLE, true),
                (bool_setting::MULTIPOINT_ENABLE, true),
                (bool_setting::VOLUME_EQ_ENABLE, false),
                (bool_setting::SUM_TO_MONO, false),
                (bool_setting::SPEECH_DETECTION, false),
            ]),
            eq: proto::EqBands::from_gains([1.0, 0.5, 0.0, -1.5, 2.0]),
            last_saved: proto::EqBands::from_gains([2.0, 1.0, 0.0, 1.0, 2.0]),
            volume_asymmetry: 0,
            gesture_control: proto::GestureControl::new(
                proto::action_target::ANC_CONTROL,
                proto::action_target::ANC_CONTROL,
            ),
            anc_loop: proto::AncrGestureLoop {
                active: true,
                off: false,
                aware: true,
                adaptive: false,
            },
            #[cfg(test)]
            refuse_last_saved: false,
            battery: proto::BatteryInfo {
                left: battery(71, false),
                right: battery(69, false),
                case: battery(45, true),
            },
            runtime_subscribed: false,
            settings_subscribed: false,
            #[cfg(test)]
            reject_writes: false,
        }
    }

    #[cfg(test)]
    pub fn subscriptions(&self) -> (bool, bool) {
        (self.runtime_subscribed, self.settings_subscribed)
    }

    #[cfg(test)]
    pub fn anc(&self) -> i32 {
        self.anc
    }

    #[cfg(test)]
    pub fn hold_targets(&self) -> (i32, i32) {
        self.gesture_control.targets()
    }

    #[cfg(test)]
    pub fn volume_asymmetry(&self) -> i32 {
        self.volume_asymmetry
    }

    #[cfg(test)]
    pub fn eq(&self) -> [f32; 5] {
        self.eq.gains()
    }

    #[cfg(test)]
    pub fn refuse_last_saved(&mut self) {
        self.refuse_last_saved = true;
    }

    #[cfg(test)]
    pub fn reject_writes(&mut self) {
        self.reject_writes = true;
    }

    /// Bytes the host writes. Returns the bytes the earbuds send back.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.rx.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut offset = 0;
        while let Some((consumed, frame)) = hdlc::decode(&self.rx[offset..]) {
            offset += consumed;
            // Like real firmware, ignore anything that doesn't decode.
            if let Ok(Some(frame)) = frame
                && let Ok(packet) = rpc::decode_packet(&frame)
            {
                packets.push(packet);
            }
        }
        self.rx.drain(..offset);
        packets.into_iter().flat_map(|p| self.handle(p)).collect()
    }

    /// Simulate time passing: each bud not charging loses 1% battery. Returns
    /// the runtime-info update the earbuds stream, if the host subscribed.
    pub fn tick(&mut self) -> Vec<u8> {
        for bud in [&mut self.battery.left, &mut self.battery.right]
            .into_iter()
            .flatten()
        {
            if bud.state != battery_state::CHARGING {
                bud.level = (bud.level - 1).max(0);
            }
        }
        if !self.runtime_subscribed {
            return vec![];
        }
        self.stream_packet(Method::SubscribeRuntimeInfo, self.runtime_info())
    }

    /// Answers one request. Never panics: the app's Debug menu runs this
    /// simulator, so malformed input gets an RPC error, as from real earbuds.
    fn handle(&mut self, request: RpcPacket) -> Vec<u8> {
        if request.packet_type != packet_type::REQUEST {
            return vec![]; // E.g. a client error cancelling a call.
        }
        let reply = |packet_type: i32, status: u32, payload: Vec<u8>| {
            rpc::encode_packet(&RpcPacket {
                packet_type,
                status,
                payload,
                ..request.clone()
            })
        };
        if request.channel_id != self.channel {
            return reply(packet_type::SERVER_ERROR, STATUS_NOT_FOUND, vec![]);
        }

        let error = |status: u32| {
            rpc::encode_packet(&RpcPacket {
                packet_type: packet_type::SERVER_ERROR,
                status,
                payload: vec![],
                ..request.clone()
            })
        };
        let Some(method) = Method::from_packet(&request) else {
            return error(STATUS_UNIMPLEMENTED);
        };
        match method {
            Method::GetSoftwareInfo => reply(packet_type::RESPONSE, 0, software_info()),
            Method::GetHardwareInfo => reply(packet_type::RESPONSE, 0, hardware_info()),
            Method::SubscribeRuntimeInfo => {
                self.runtime_subscribed = true;
                reply(packet_type::SERVER_STREAM, 0, self.runtime_info())
            }
            Method::SubscribeToSettingsChanges => {
                self.settings_subscribed = true;
                vec![]
            }
            Method::ReadSetting => {
                let Ok(msg) = proto::ReadSettingMsg::decode(request.payload.as_slice()) else {
                    return error(STATUS_INVALID_ARGUMENT);
                };
                let setting = match msg.settings_id {
                    Some(setting_id::CURRENT_ANCR_STATE) => Setting::CurrentAncrState(self.anc),
                    Some(setting_id::CURRENT_USER_EQ) => Setting::CurrentUserEq(self.eq),
                    Some(setting_id::VOLUME_ASYMMETRY) => {
                        Setting::VolumeAsymmetry(self.volume_asymmetry)
                    }
                    Some(setting_id::ANCR_GESTURE_LOOP) => Setting::AncrGestureLoop(self.anc_loop),
                    Some(setting_id::GESTURE_CONTROL) => {
                        Setting::GestureControl(self.gesture_control.clone())
                    }
                    Some(setting_id::LAST_SAVED_USER_EQ) => {
                        #[cfg(test)]
                        if self.refuse_last_saved {
                            return error(STATUS_UNKNOWN);
                        }
                        Setting::LastSavedUserEq(self.last_saved)
                    }
                    Some(id) => match self
                        .bools
                        .get(&id)
                        .and_then(|&on| Setting::from_bool(id, on))
                    {
                        Some(setting) => setting,
                        None => return error(STATUS_NOT_FOUND),
                    },
                    None => return error(STATUS_INVALID_ARGUMENT),
                };
                reply(packet_type::RESPONSE, 0, settings_rsp(setting))
            }
            Method::WriteSetting => {
                #[cfg(test)]
                if self.reject_writes {
                    return reply(packet_type::RESPONSE, STATUS_FAILED_PRECONDITION, vec![]);
                }
                let setting = proto::WriteSettingMsg::decode(request.payload.as_slice())
                    .ok()
                    .and_then(|msg| msg.setting)
                    .and_then(|s| s.value);
                let Some(setting) = setting else {
                    return error(STATUS_INVALID_ARGUMENT);
                };
                // Like the real earbuds, a write merges into the stored curve
                // (missing bands stay as they were), and the change is reported
                // as the full resulting curve.
                let setting = match setting {
                    Setting::CurrentUserEq(bands) => Setting::CurrentUserEq(self.eq.merged(&bands)),
                    other => other,
                };
                match setting {
                    Setting::CurrentAncrState(state) => self.anc = state,
                    Setting::CurrentUserEq(bands) => self.eq = bands,
                    Setting::LastSavedUserEq(bands) => self.last_saved = bands,
                    Setting::VolumeAsymmetry(raw) => self.volume_asymmetry = raw,
                    Setting::AncrGestureLoop(anc_loop) => {
                        // Like the Pixel Buds app, refuse fewer than two modes.
                        let on = [
                            anc_loop.active,
                            anc_loop.off,
                            anc_loop.aware,
                            anc_loop.adaptive,
                        ];
                        if on.iter().filter(|&&b| b).count() < 2 {
                            return error(STATUS_INVALID_ARGUMENT);
                        }
                        self.anc_loop = anc_loop;
                    }
                    Setting::GestureControl(ref control) => {
                        self.gesture_control = control.clone();
                    }
                    ref other => match other.as_bool() {
                        Some((id, on)) => {
                            self.bools.insert(id, on);
                        }
                        None => return error(STATUS_INVALID_ARGUMENT),
                    },
                }
                let mut out = reply(packet_type::RESPONSE, 0, vec![]);
                out.extend(self.settings_changed(setting));
                out
            }
        }
    }

    fn stream_packet(&self, method: Method, payload: Vec<u8>) -> Vec<u8> {
        rpc::encode_packet(&RpcPacket {
            packet_type: packet_type::SERVER_STREAM,
            channel_id: self.channel,
            service_id: MAESTRO_SERVICE,
            method_id: method.id(),
            payload,
            status: 0,
            call_id: 0,
        })
    }

    fn settings_changed(&self, setting: Setting) -> Vec<u8> {
        if !self.settings_subscribed {
            return vec![];
        }
        self.stream_packet(Method::SubscribeToSettingsChanges, settings_rsp(setting))
    }

    fn runtime_info(&self) -> Vec<u8> {
        proto::RuntimeInfo {
            battery_info: Some(self.battery.clone()),
        }
        .encode_to_vec()
    }

    /// The ANC mode changes on the earbuds themselves (e.g. a long press).
    #[cfg(test)]
    pub fn external_anc_change(&mut self, state: i32) -> Vec<u8> {
        if state != 4 {
            self.anc = state;
        }
        self.settings_changed(Setting::CurrentAncrState(state))
    }

    /// Another device (e.g. the user's phone) changes the EQ.
    #[cfg(test)]
    pub fn external_eq_change(&mut self, gains: [f32; 5]) -> Vec<u8> {
        self.eq = proto::EqBands::from_gains(gains);
        self.settings_changed(Setting::CurrentUserEq(self.eq))
    }

    /// Both buds report `level`; the case reports `case` (`None`: not reporting).
    #[cfg(test)]
    pub fn battery_update(&mut self, level: i32, case: Option<i32>) -> Vec<u8> {
        self.battery = proto::BatteryInfo {
            left: battery(level, false),
            right: battery(level, false),
            case: case.and_then(|level| battery(level, true)),
        };
        self.stream_packet(Method::SubscribeRuntimeInfo, self.runtime_info())
    }
}

fn settings_rsp(setting: Setting) -> Vec<u8> {
    SettingsRsp {
        value: Some(SettingValue {
            value: Some(setting),
        }),
    }
    .encode_to_vec()
}

/// The firmware your Buds Pro reported: build "575", "release_5.11_signed".
fn software_info() -> Vec<u8> {
    let version = || {
        Some(FirmwareVersion {
            build: "575".to_string(),
            version_string: "release_5.11_signed".to_string(),
        })
    };
    SoftwareInfo {
        firmware: Some(FirmwareInfo {
            case: version(),
            right: version(),
            left: version(),
        }),
    }
    .encode_to_vec()
}

/// Made-up serial numbers.
fn hardware_info() -> Vec<u8> {
    HardwareInfo {
        serial_numbers: Some(SerialNumbers {
            case: "SIMCASE01".to_string(),
            right: "SIMRIGHT01".to_string(),
            left: "SIMLEFT01".to_string(),
        }),
    }
    .encode_to_vec()
}

impl crate::simulator::Simulator for PixelBudsSim {
    fn receive(&mut self, bytes: &[u8]) -> Vec<u8> {
        PixelBudsSim::receive(self, bytes)
    }

    fn tick(&mut self) -> Vec<u8> {
        PixelBudsSim::tick(self)
    }
}
