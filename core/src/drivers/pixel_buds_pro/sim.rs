//! A simulated pair of Pixel Buds Pro for tests: just enough of the Maestro
//! server side to exercise the driver end to end.

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

/// pw_rpc's NOT_FOUND, sent for requests on channels the device doesn't serve.
const STATUS_NOT_FOUND: u32 = 5;
const STATUS_FAILED_PRECONDITION: u32 = 9;

pub(crate) struct PixelBudsSim {
    channel: u32,
    rx: Vec<u8>,
    anc: i32,
    eq: proto::EqBands,
    /// On/off settings by ID.
    bools: HashMap<i32, bool>,
    battery: proto::BatteryInfo,
    runtime_subscribed: bool,
    settings_subscribed: bool,
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
            battery: proto::BatteryInfo {
                left: battery(71, false),
                right: battery(69, false),
                case: battery(45, true),
            },
            runtime_subscribed: false,
            settings_subscribed: false,
            reject_writes: false,
        }
    }

    pub fn subscriptions(&self) -> (bool, bool) {
        (self.runtime_subscribed, self.settings_subscribed)
    }

    pub fn anc(&self) -> i32 {
        self.anc
    }

    pub fn eq(&self) -> [f32; 5] {
        self.eq.gains()
    }

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
            if let Ok(Some(frame)) = frame {
                packets.push(rpc::decode_packet(&frame).expect("driver sends valid packets"));
            }
        }
        self.rx.drain(..offset);
        packets.into_iter().flat_map(|p| self.handle(p)).collect()
    }

    fn handle(&mut self, request: RpcPacket) -> Vec<u8> {
        assert_eq!(
            request.packet_type,
            packet_type::REQUEST,
            "host sends only requests"
        );
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

        match Method::from_packet(&request).expect("driver calls known methods") {
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
                let msg = proto::ReadSettingMsg::decode(request.payload.as_slice()).unwrap();
                let setting = match msg.settings_id {
                    Some(setting_id::CURRENT_ANCR_STATE) => Setting::CurrentAncrState(self.anc),
                    Some(setting_id::CURRENT_USER_EQ) => Setting::CurrentUserEq(self.eq),
                    Some(id) if self.bools.contains_key(&id) => {
                        Setting::from_bool(id, self.bools[&id]).unwrap()
                    }
                    other => panic!("unexpected read of setting {other:?}"),
                };
                reply(packet_type::RESPONSE, 0, settings_rsp(setting))
            }
            Method::WriteSetting => {
                if self.reject_writes {
                    return reply(packet_type::RESPONSE, STATUS_FAILED_PRECONDITION, vec![]);
                }
                let msg = proto::WriteSettingMsg::decode(request.payload.as_slice()).unwrap();
                let setting = msg
                    .setting
                    .and_then(|s| s.value)
                    .expect("write carries a setting");
                match setting {
                    Setting::CurrentAncrState(state) => self.anc = state,
                    Setting::CurrentUserEq(bands) => self.eq = bands,
                    ref other => {
                        let (id, on) = other.as_bool().unwrap();
                        self.bools.insert(id, on);
                    }
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
    pub fn external_anc_change(&mut self, state: i32) -> Vec<u8> {
        if state != 4 {
            self.anc = state;
        }
        self.settings_changed(Setting::CurrentAncrState(state))
    }

    /// Another device (e.g. the user's phone) changes the EQ.
    pub fn external_eq_change(&mut self, gains: [f32; 5]) -> Vec<u8> {
        self.eq = proto::EqBands::from_gains(gains);
        self.settings_changed(Setting::CurrentUserEq(self.eq))
    }

    /// Both buds report `level`; the case reports `case` (`None`: not reporting).
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
