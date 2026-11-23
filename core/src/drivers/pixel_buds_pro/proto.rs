//! The subset of the `maestro_pw` protobuf messages this driver uses.
//!
//! Field numbers come from pbpctrl's reverse-engineered `maestro_pw.proto`.
//! Fields we don't use are omitted; protobuf decoding skips them.

use prost::{Message, Oneof};

pub(crate) use crate::drivers::google::RuntimeInfo;
#[cfg(test)]
pub(crate) use crate::drivers::google::{BatteryInfo, DeviceBatteryInfo, battery_state};

/// `AllegroSettingType`: which setting `ReadSetting` should return.
pub(crate) mod setting_id {
    pub const CURRENT_ANCR_STATE: i32 = 13;
    pub const CURRENT_USER_EQ: i32 = 16;
}

/// `AncState`, the value of the `current_ancr_state` setting.
pub(crate) mod anc_state {
    /// Reported while both buds are in the case.
    pub const UNKNOWN: i32 = 0;
    pub const OFF: i32 = 1;
    pub const ACTIVE: i32 = 2;
    pub const AWARE: i32 = 3;
}

/// `ReadSettingMsg`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct ReadSettingMsg {
    #[prost(int32, optional, tag = "4")]
    pub settings_id: Option<i32>,
}

/// `WriteSettingMsg`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct WriteSettingMsg {
    #[prost(message, optional, tag = "4")]
    pub setting: Option<SettingValue>,
}

/// `SettingsRsp`: the reply to `ReadSetting`, and each `SubscribeToSettingsChanges` message.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct SettingsRsp {
    #[prost(message, optional, tag = "4")]
    pub value: Option<SettingValue>,
}

/// `SettingValue`, a oneof over every setting. Settings we don't handle
/// decode as `value: None`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct SettingValue {
    #[prost(oneof = "Setting", tags = "13, 16")]
    pub value: Option<Setting>,
}

#[derive(Clone, PartialEq, Oneof)]
pub(crate) enum Setting {
    /// An [`anc_state`] value.
    #[prost(int32, tag = "13")]
    CurrentAncrState(i32),
    #[prost(message, tag = "16")]
    CurrentUserEq(EqBands),
}

/// `EqBands`: gains in dB, from -6.0 to 6.0.
#[derive(Clone, Copy, PartialEq, Message)]
pub(crate) struct EqBands {
    #[prost(float, tag = "1")]
    pub low_bass: f32,
    #[prost(float, tag = "2")]
    pub bass: f32,
    #[prost(float, tag = "3")]
    pub mid: f32,
    #[prost(float, tag = "4")]
    pub treble: f32,
    #[prost(float, tag = "5")]
    pub upper_treble: f32,
}

impl EqBands {
    pub fn from_gains(g: [f32; 5]) -> Self {
        Self {
            low_bass: g[0],
            bass: g[1],
            mid: g[2],
            treble: g[3],
            upper_treble: g[4],
        }
    }

    pub fn gains(&self) -> [f32; 5] {
        [
            self.low_bass,
            self.bass,
            self.mid,
            self.treble,
            self.upper_treble,
        ]
    }
}

pub(crate) fn write_setting(setting: Setting) -> Vec<u8> {
    WriteSettingMsg {
        setting: Some(SettingValue {
            value: Some(setting),
        }),
    }
    .encode_to_vec()
}

pub(crate) fn read_setting(id: i32) -> Vec<u8> {
    ReadSettingMsg {
        settings_id: Some(id),
    }
    .encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_setting_wire_format() {
        // Field 4, varint: tag byte (4 << 3 | 0) = 0x20, then 13.
        assert_eq!(read_setting(setting_id::CURRENT_ANCR_STATE), [0x20, 0x0D]);
    }

    #[test]
    fn write_anc_wire_format() {
        // WriteSettingMsg.setting (field 4, length-delimited) =
        //   SettingValue.current_ancr_state (field 13, varint) = 2.
        assert_eq!(
            write_setting(Setting::CurrentAncrState(anc_state::ACTIVE)),
            [0x22, 0x02, 0x68, 0x02]
        );
    }

    #[test]
    fn unknown_settings_decode_as_none() {
        // SettingsRsp.value = SettingValue.multipoint_enable (field 11) = true.
        let rsp = SettingsRsp::decode(&[0x22, 0x02, 0x58, 0x01][..]).unwrap();
        assert_eq!(rsp.value, Some(SettingValue { value: None }));
    }
}
