//! The internal contract every vendor driver implements. Not exported over FFI:
//! Swift talks to [`crate::BarBudsController`], which owns a driver.

use crate::error::BarBudsError;
use crate::model::{AncMode, BatteryStatus, DeviceCapabilities, EqSettings};

/// A state change decoded from one inbound frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Battery(BatteryStatus),
    Anc(AncMode),
    Eq(EqSettings),
}

/// The outcome of decoding the frame at the front of a receive buffer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decoded {
    /// Bytes to drop from the front of the buffer. Always > 0, including on
    /// errors, so the caller can skip past garbage and resynchronize.
    pub consumed: usize,
    pub result: Result<Event, BarBudsError>,
}

pub(crate) trait EarbudDriver: Send + Sync {
    fn capabilities(&self) -> DeviceCapabilities;

    /// Decode one frame from the front of `buf`. Returns `None` when `buf`
    /// does not yet hold a complete frame. Bluetooth reads may split or
    /// merge frames arbitrarily.
    fn parse_frame(&self, buf: &[u8]) -> Option<Decoded>;

    /// Frame(s) asking the device to report its full state. Sent after connecting.
    fn encode_status_request(&self) -> Vec<u8>;

    fn encode_anc(&self, mode: AncMode) -> Result<Vec<u8>, BarBudsError>;

    fn encode_eq(&self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError>;
}
