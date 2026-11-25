//! The internal contract every vendor driver implements. Not exported over FFI:
//! Swift talks to [`crate::BarBudsController`], which owns a driver.

use crate::error::BarBudsError;
use crate::model::{
    AncAvailability, AncMode, BatteryStatus, ComponentStrings, ControlSetting, DeviceCapabilities,
    EqSettings, FeatureToggle, SpatialAudioMode,
};

/// A state change decoded from one inbound frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Battery(BatteryStatus),
    /// `None`: the device can't say right now (e.g. Pixel Buds in their case).
    Anc(Option<AncMode>),
    Eq(EqSettings),
    Toggle(FeatureToggle, bool),
    Firmware(ComponentStrings),
    /// See [`crate::DeviceState::balance`].
    Balance(i32),
    SerialNumbers(ComponentStrings),
    /// Gesture actions; gestures not listed keep their last value.
    Controls(Vec<ControlSetting>),
    /// The noise-control mode and whether BarBuds can change it.
    AncState(Option<AncMode>, AncAvailability),
    AncAvailability(AncAvailability),
    SpatialAudio(SpatialAudioMode),
    /// The noise-control gesture's modes, in capabilities order.
    AncCycle(Vec<AncMode>),
}

/// The outcome of decoding the frame at the front of a receive buffer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decoded {
    /// Bytes to drop from the front of the buffer. Always > 0, including on
    /// errors, so the caller can skip past garbage and resynchronize.
    pub consumed: usize,
    /// `Ok(None)` for frames that are valid but don't change the UI state,
    /// such as handshake replies.
    pub result: Result<Option<Event>, BarBudsError>,
}

/// One driver instance serves one connection, so drivers may keep protocol
/// state (negotiated channels, requests in flight). The controller replaces
/// the driver with a fresh one on `reset()`.
pub(crate) trait EarbudDriver: Send {
    fn capabilities(&self) -> DeviceCapabilities;

    /// Frame(s) to write once the channel opens: a handshake or a request for
    /// the device's full state.
    fn start(&mut self) -> Vec<u8>;

    /// Decode one frame from the front of `buf`. Returns `None` when `buf`
    /// does not yet hold a complete frame. Bluetooth reads may split or
    /// merge frames arbitrarily.
    ///
    /// Protocols with a multi-step handshake append follow-up requests to
    /// `outbound`, and the controller hands them to Swift to write.
    fn parse_frame(&mut self, buf: &[u8], outbound: &mut Vec<u8>) -> Option<Decoded>;

    fn encode_anc(&mut self, mode: AncMode) -> Result<Vec<u8>, BarBudsError>;

    fn encode_eq(&mut self, eq: &EqSettings) -> Result<Vec<u8>, BarBudsError>;

    /// The device's Bluetooth name, given before `start()` (and kept across
    /// reconnects). Drivers covering several models may use it to identify
    /// the model; most ignore it.
    fn set_device_name(&mut self, name: &str) {
        let _ = name;
    }

    /// Only called when `capabilities().has_balance`, with a value already
    /// checked to be in `BALANCE_MIN..=BALANCE_MAX`.
    fn encode_balance(&mut self, balance: i32) -> Result<Vec<u8>, BarBudsError> {
        let _ = balance;
        Err(BarBudsError::Unsupported {
            feature: "balance".to_string(),
        })
    }

    /// Only called for a slot listed in `capabilities().controls` and one
    /// of that slot's actions.
    fn encode_control(&mut self, setting: ControlSetting) -> Result<Vec<u8>, BarBudsError> {
        let _ = setting;
        Err(BarBudsError::Unsupported {
            feature: "gesture controls".to_string(),
        })
    }

    /// Only called with at least `ANC_CYCLE_MIN_MODES` distinct modes, all
    /// from `capabilities().anc_cycle_modes`, in that order.
    fn encode_anc_cycle(&mut self, modes: &[AncMode]) -> Result<Vec<u8>, BarBudsError> {
        let _ = modes;
        Err(BarBudsError::Unsupported {
            feature: "the noise-control cycle".to_string(),
        })
    }

    /// Only called with a mode listed in `capabilities().spatial_audio_modes`.
    fn encode_spatial_audio(&mut self, mode: SpatialAudioMode) -> Result<Vec<u8>, BarBudsError> {
        let _ = mode;
        Err(BarBudsError::Unsupported {
            feature: "spatial audio".to_string(),
        })
    }

    /// Only called for toggles listed in `capabilities().toggles`.
    fn encode_toggle(&mut self, toggle: FeatureToggle, on: bool) -> Result<Vec<u8>, BarBudsError> {
        let _ = on;
        Err(BarBudsError::Unsupported {
            feature: format!("{toggle:?}"),
        })
    }
}
