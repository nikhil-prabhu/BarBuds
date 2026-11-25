//! The subset of the `maestro_pw` protobuf messages this driver uses.
//!
//! Field numbers come from pbpctrl's reverse-engineered `maestro_pw.proto`.
//! Fields we don't use are omitted; protobuf decoding skips them.

use prost::{Message, Oneof};

pub(crate) use crate::drivers::google::RuntimeInfo;
pub(crate) use crate::drivers::google::{BatteryInfo, DeviceBatteryInfo, battery_state};

/// `AllegroSettingType`: which setting `ReadSetting` should return.
pub(crate) mod setting_id {
    pub const CURRENT_ANCR_STATE: i32 = 13;
    pub const CURRENT_USER_EQ: i32 = 16;
    /// The user's saved custom curve ("Last Saved" in the Pixel Buds app).
    /// pbpctrl saw reads fail on its firmware; BudsLink reads it fine.
    pub const LAST_SAVED_USER_EQ: i32 = 18;
}

/// The on/off settings BarBuds shows, by `AllegroSettingType` ID. Each is a
/// `bool` field of `SettingValue` with the same number.
pub(crate) mod bool_setting {
    /// On-head (in-ear) detection.
    pub const OHD_ENABLE: i32 = 2;
    /// Touch gestures.
    pub const GESTURE_ENABLE: i32 = 4;
    pub const MULTIPOINT_ENABLE: i32 = 11;
    pub const VOLUME_EQ_ENABLE: i32 = 15;
    /// Mono audio.
    pub const SUM_TO_MONO: i32 = 19;
    /// Conversation Detection.
    pub const SPEECH_DETECTION: i32 = 22;
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
    #[prost(oneof = "Setting", tags = "2, 4, 11, 13, 15, 16, 18, 19, 22")]
    pub value: Option<Setting>,
}

#[derive(Clone, PartialEq, Oneof)]
pub(crate) enum Setting {
    #[prost(bool, tag = "2")]
    OhdEnable(bool),
    #[prost(bool, tag = "4")]
    GestureEnable(bool),
    #[prost(bool, tag = "11")]
    MultipointEnable(bool),
    #[prost(bool, tag = "15")]
    VolumeEqEnable(bool),
    #[prost(bool, tag = "19")]
    SumToMono(bool),
    #[prost(bool, tag = "22")]
    SpeechDetection(bool),
    /// An [`anc_state`] value.
    #[prost(int32, tag = "13")]
    CurrentAncrState(i32),
    #[prost(message, tag = "16")]
    CurrentUserEq(EqBands),
    #[prost(message, tag = "18")]
    LastSavedUserEq(EqBands),
}

impl Setting {
    /// The setting for [`bool_setting`] `id`, or `None` if `id` isn't one.
    pub fn from_bool(id: i32, on: bool) -> Option<Self> {
        Some(match id {
            bool_setting::OHD_ENABLE => Setting::OhdEnable(on),
            bool_setting::GESTURE_ENABLE => Setting::GestureEnable(on),
            bool_setting::MULTIPOINT_ENABLE => Setting::MultipointEnable(on),
            bool_setting::VOLUME_EQ_ENABLE => Setting::VolumeEqEnable(on),
            bool_setting::SUM_TO_MONO => Setting::SumToMono(on),
            bool_setting::SPEECH_DETECTION => Setting::SpeechDetection(on),
            _ => return None,
        })
    }

    /// The [`bool_setting`] ID and value, for on/off settings.
    pub fn as_bool(&self) -> Option<(i32, bool)> {
        Some(match *self {
            Setting::OhdEnable(on) => (bool_setting::OHD_ENABLE, on),
            Setting::GestureEnable(on) => (bool_setting::GESTURE_ENABLE, on),
            Setting::MultipointEnable(on) => (bool_setting::MULTIPOINT_ENABLE, on),
            Setting::VolumeEqEnable(on) => (bool_setting::VOLUME_EQ_ENABLE, on),
            Setting::SumToMono(on) => (bool_setting::SUM_TO_MONO, on),
            Setting::SpeechDetection(on) => (bool_setting::SPEECH_DETECTION, on),
            Setting::CurrentAncrState(_)
            | Setting::CurrentUserEq(_)
            | Setting::LastSavedUserEq(_) => return None,
        })
    }
}

/// `EqBands`: gains in dB, from -6.0 to 6.0.
///
/// The fields are `optional` so every band is sent, zeros included, as the
/// Pixel Buds app does: the earbuds treat a missing band as "unchanged", so
/// leaving out a 0 dB band would keep its previous value (seen on real Pixel
/// Buds Pro, 2026-11-25).
#[derive(Clone, Copy, PartialEq, Message)]
pub(crate) struct EqBands {
    #[prost(float, optional, tag = "1")]
    pub low_bass: Option<f32>,
    #[prost(float, optional, tag = "2")]
    pub bass: Option<f32>,
    #[prost(float, optional, tag = "3")]
    pub mid: Option<f32>,
    #[prost(float, optional, tag = "4")]
    pub treble: Option<f32>,
    #[prost(float, optional, tag = "5")]
    pub upper_treble: Option<f32>,
}

impl EqBands {
    /// All five bands, each present.
    pub fn from_gains(g: [f32; 5]) -> Self {
        Self {
            low_bass: Some(g[0]),
            bass: Some(g[1]),
            mid: Some(g[2]),
            treble: Some(g[3]),
            upper_treble: Some(g[4]),
        }
    }

    /// The gains, reading a missing band as 0 dB. (The earbuds always report
    /// all five.)
    pub fn gains(&self) -> [f32; 5] {
        self.fields().map(|band| band.unwrap_or(0.0))
    }

    fn fields(&self) -> [Option<f32>; 5] {
        [
            self.low_bass,
            self.bass,
            self.mid,
            self.treble,
            self.upper_treble,
        ]
    }

    /// `self` updated by `update` the way the earbuds apply a write: bands
    /// missing from `update` keep their current value.
    pub fn merged(&self, update: &EqBands) -> EqBands {
        let current = self.fields();
        let new = update.fields();
        let pick = |i: usize| new[i].or(current[i]);
        EqBands {
            low_bass: pick(0),
            bass: pick(1),
            mid: pick(2),
            treble: pick(3),
            upper_treble: pick(4),
        }
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
    fn bool_settings_round_trip() {
        for id in [2, 4, 11, 15, 19, 22] {
            let setting = Setting::from_bool(id, true).unwrap();
            assert_eq!(setting.as_bool(), Some((id, true)));
            let bytes = write_setting(setting.clone());
            let msg = WriteSettingMsg::decode(bytes.as_slice()).unwrap();
            assert_eq!(msg.setting.unwrap().value, Some(setting), "setting {id}");
        }
        assert_eq!(Setting::from_bool(13, true), None);
    }

    #[test]
    fn eq_writes_include_zero_bands() {
        // The Pixel Buds app's Vocal Boost write, captured on a phone
        // (2026-11-25): every band present, zeros included.
        let expected = [
            0x0d, 0x00, 0x00, 0x80, 0xbf, // low bass -1.0
            0x15, 0x00, 0x00, 0x00, 0x00, // bass 0.0
            0x1d, 0x00, 0x00, 0x80, 0x40, // mid 4.0
            0x25, 0x00, 0x00, 0x00, 0x40, // treble 2.0
            0x2d, 0x00, 0x00, 0x00, 0x00, // upper treble 0.0
        ];
        let bands = EqBands::from_gains([-1.0, 0.0, 4.0, 2.0, 0.0]);
        assert_eq!(bands.encode_to_vec(), expected);
    }

    #[test]
    fn missing_bands_keep_their_value_when_merged() {
        let balanced = EqBands::from_gains([-3.5, 0.5, 1.0, -1.0, 2.5]);
        // What the old encoder sent for Heavy Bass: only the non-zero bands.
        let partial = EqBands {
            low_bass: Some(5.0),
            bass: Some(3.0),
            ..Default::default()
        };
        // The mix seen on real earbuds.
        assert_eq!(
            balanced.merged(&partial).gains(),
            [5.0, 3.0, 1.0, -1.0, 2.5]
        );
        let heavy = EqBands::from_gains([5.0, 3.0, 0.0, 0.0, 0.0]);
        assert_eq!(balanced.merged(&heavy).gains(), [5.0, 3.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn unknown_settings_decode_as_none() {
        // SettingsRsp.value = SettingValue.volume_exposure_notifications
        // (field 21, key 21 << 3 = 0xA8 0x01) = true.
        let rsp = SettingsRsp::decode(&[0x22, 0x03, 0xA8, 0x01, 0x01][..]).unwrap();
        assert_eq!(rsp.value, Some(SettingValue { value: None }));
    }
}
