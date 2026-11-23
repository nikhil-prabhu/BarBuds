//! Protobuf messages shared by Google's earbud protocols (Maestro on the
//! Pixel Buds Pro, and the A-Series control channel).

use prost::Message;

use crate::model::{BatteryReading, BatteryStatus};

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
    #[cfg_attr(not(test), allow(dead_code))] // The driver treats any other known state as not charging.
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
