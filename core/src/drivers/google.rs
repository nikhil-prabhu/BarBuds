//! Protobuf messages shared by Google's earbud protocols (Maestro on the
//! Pixel Buds Pro, and the A-Series control channel).

use prost::Message;

use crate::model::{BatteryReading, BatteryStatus, ComponentStrings};

/// `SoftwareInfo`: Maestro's `GetSoftwareInfo` reply; the A-Series sends it
/// as message `87 02`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct SoftwareInfo {
    #[prost(message, optional, tag = "4")]
    pub firmware: Option<FirmwareInfo>,
}

/// One entry per component. The field order (case, right, left) is
/// confirmed by the A-Series serial numbers, which end in `UR` and `UL`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct FirmwareInfo {
    #[prost(message, optional, tag = "1")]
    pub case: Option<FirmwareVersion>,
    #[prost(message, optional, tag = "2")]
    pub right: Option<FirmwareVersion>,
    #[prost(message, optional, tag = "3")]
    pub left: Option<FirmwareVersion>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct FirmwareVersion {
    /// A build number: "575" on the Buds Pro, "300527" on the A-Series.
    #[prost(string, tag = "1")]
    pub build: String,
    /// The displayed version, e.g. "release_5.11_signed". Absent on the A-Series.
    #[prost(string, tag = "2")]
    pub version_string: String,
}

/// `HardwareInfo`: Maestro's `GetHardwareInfo` reply; the A-Series sends it
/// as message `87 05`.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct HardwareInfo {
    #[prost(message, optional, tag = "7")]
    pub serial_numbers: Option<SerialNumbers>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct SerialNumbers {
    #[prost(string, tag = "1")]
    pub case: String,
    #[prost(string, tag = "2")]
    pub right: String,
    #[prost(string, tag = "3")]
    pub left: String,
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Firmware per component, formatted by `display` (which returns `None` to
/// leave a component out).
pub(crate) fn firmware(
    info: &SoftwareInfo,
    display: impl Fn(&FirmwareVersion) -> Option<String>,
) -> Option<ComponentStrings> {
    let firmware = info.firmware.as_ref()?;
    let version = |v: &Option<FirmwareVersion>| v.as_ref().and_then(&display);
    Some(ComponentStrings {
        left: version(&firmware.left),
        right: version(&firmware.right),
        charging_case: version(&firmware.case),
    })
}

pub(crate) fn serial_numbers(info: &HardwareInfo) -> Option<ComponentStrings> {
    let serials = info.serial_numbers.as_ref()?;
    Some(ComponentStrings {
        left: non_empty(&serials.left),
        right: non_empty(&serials.right),
        charging_case: non_empty(&serials.case),
    })
}

/// The displayed version string, else the build number.
pub(crate) fn version_string_or_build(v: &FirmwareVersion) -> Option<String> {
    non_empty(&v.version_string).or_else(|| non_empty(&v.build))
}

/// `RuntimeInfo`: Maestro streams it from `SubscribeRuntimeInfo`; the
/// A-Series sends it as message `87 06`. Same field numbers in both.
#[derive(Clone, PartialEq, Message)]
pub(crate) struct RuntimeInfo {
    #[prost(message, optional, tag = "6")]
    pub battery_info: Option<BatteryInfo>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct BatteryInfo {
    #[prost(message, optional, tag = "1")]
    pub case: Option<DeviceBatteryInfo>,
    #[prost(message, optional, tag = "2")]
    pub left: Option<DeviceBatteryInfo>,
    #[prost(message, optional, tag = "3")]
    pub right: Option<DeviceBatteryInfo>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct DeviceBatteryInfo {
    /// Percent.
    #[prost(int32, tag = "1")]
    pub level: i32,
    /// See [`battery_state`].
    #[prost(int32, tag = "2")]
    pub state: i32,
}

pub(crate) mod battery_state {
    pub const UNKNOWN: i32 = 0;
    pub const NOT_CHARGING: i32 = 1;
    pub const CHARGING: i32 = 2;
}

fn battery_reading(info: &Option<DeviceBatteryInfo>) -> Option<BatteryReading> {
    let info = info.as_ref()?;
    if info.state == battery_state::UNKNOWN {
        return None;
    }
    Some(BatteryReading {
        percent: info.level.clamp(0, 100) as u8,
        charging: info.state == battery_state::CHARGING,
    })
}

pub(crate) fn battery_status(info: &BatteryInfo) -> BatteryStatus {
    BatteryStatus {
        left: battery_reading(&info.left),
        right: battery_reading(&info.right),
        charging_case: battery_reading(&info.case),
    }
}
