//! Vendor-neutral device state shared with Swift. Every driver translates its
//! wire format into these types, so the UI never sees vendor specifics.

/// A supported earbud model. Each variant maps to one driver in `drivers/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum DeviceModel {
    /// Simulated earbuds speaking a made-up protocol, for UI development.
    Mock,
    /// Google Pixel Buds Pro (1st generation), via the Maestro protocol.
    PixelBudsPro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AncMode {
    Off,
    NoiseCancelling,
    Transparency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum EqPreset {
    Flat,
    BassBoost,
    Vocal,
    TrebleBoost,
    /// User-defined curve; the band gains in [`EqSettings`] are used as-is.
    Custom,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqSettings {
    pub preset: EqPreset,
    /// One gain per band in [`EqCapabilities::band_labels`], in order.
    /// Ignored when encoding a non-`Custom` preset; the driver applies the preset's curve.
    pub gains_db: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct BatteryReading {
    /// 0–100.
    pub percent: u8,
    pub charging: bool,
}

/// `None` means the component is not reporting (e.g. a bud is in the case and
/// the case lid is closed, or the device has no case).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct BatteryStatus {
    pub left: Option<BatteryReading>,
    pub right: Option<BatteryReading>,
    pub charging_case: Option<BatteryReading>,
}

/// Static description of what a model supports, so the UI can render only the
/// controls that make sense for it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceCapabilities {
    pub display_name: String,
    /// Empty when the model has no ANC control.
    pub anc_modes: Vec<AncMode>,
    /// `None` when the model has no EQ control.
    pub eq: Option<EqCapabilities>,
    pub has_charging_case: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqCapabilities {
    /// Display name of each band, low to high (e.g. "60 Hz" or "Bass"). Not
    /// every vendor publishes band frequencies, so these are labels, not numbers.
    pub band_labels: Vec<String>,
    pub min_gain_db: f32,
    pub max_gain_db: f32,
    pub step_db: f32,
    pub presets: Vec<EqPreset>,
}

/// Last known state reported by the device. Fields stay `None` until the device
/// has reported them.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct DeviceState {
    pub battery: BatteryStatus,
    pub anc: Option<AncMode>,
    pub eq: Option<EqSettings>,
}
