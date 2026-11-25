//! Vendor-neutral device state shared with Swift. Every driver translates its
//! wire format into these types, so the UI never sees vendor specifics.

use std::collections::HashMap;

/// A supported earbud model. Each variant maps to one driver in `drivers/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum DeviceModel {
    /// Google Pixel Buds Pro (1st generation), via the Maestro protocol.
    PixelBudsPro,
    /// Google Pixel Buds A-Series.
    PixelBudsASeries,
    /// Nothing Ear (open), via the Nothing X control protocol.
    NothingEarOpen,
    /// Any other earbuds or headphones with Google Fast Pair: the basics
    /// (battery, firmware, noise control) over the Fast Pair message stream.
    FastPair,
}

/// Whether BarBuds can change the noise-control mode right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AncAvailability {
    #[default]
    Available,
    /// Not right now, e.g. while the earbuds aren't being worn.
    NotNow,
    /// Only on the earbuds themselves: they refuse changes from BarBuds
    /// (e.g. Fast Pair devices that require a signature only a phone can make).
    OnDeviceOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AncMode {
    Off,
    NoiseCancelling,
    Transparency,
}

/// The general shape of a model's earbuds. Generic on purpose: the UI draws a
/// matching unbranded symbol, never a picture of the actual product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FormFactor {
    /// A stem below a hard, open-fit tip.
    Stemmed,
    /// A stem below a sealed silicone tip.
    StemmedInEar,
    /// A round bud with a silicone tip and no stem.
    Stemless,
    /// An open-ear bud held by a hook around the ear.
    EarHook,
    /// Over- or on-ear headphones: one battery, no case.
    Headphones,
}

/// An on/off device feature. The UI owns titles and descriptions, so each
/// variant means the same thing for every vendor that supports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FeatureToggle {
    /// Pause playback when a bud is taken out (on-head / in-ear detection).
    InEarDetection,
    /// Raise volume automatically in noisy surroundings.
    AdaptiveSound,
    /// Boost bass and treble at low volume so music doesn't sound thin.
    VolumeEq,
    /// Pause audio when the wearer starts talking.
    ConversationDetection,
    /// Stay connected to two source devices at once.
    Multipoint,
    /// Play the same mixed-down audio in both buds.
    MonoAudio,
    /// Tap and swipe gestures on the buds.
    TouchControls,
    /// Reduce audio delay, e.g. for games and video (Nothing's "Low Lag Mode").
    LowLatency,
}

/// An EQ preset a model offers, as its vendor's app names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct EqPreset {
    /// Stable identifier, unique within the model (e.g. `"heavy-bass"`).
    pub id: String,
    /// Display name, e.g. "Heavy Bass".
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqSettings {
    /// The active preset's [`EqPreset::id`], or `None` for the user's own
    /// curve in `gains`.
    pub preset: Option<String>,
    /// One gain per band in [`EqCapabilities::band_labels`], in [`EqCapabilities::unit`].
    /// When encoding a preset, the driver applies the preset's own curve and
    /// ignores these.
    pub gains: Vec<f32>,
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
    /// The buds' general shape, so the UI can draw a matching icon.
    pub form_factor: FormFactor,
    /// On/off features the device supports, in display order.
    pub toggles: Vec<FeatureToggle>,
    /// Each bud can be rung to find it, via the Fast Pair message stream
    /// (see [`crate::FindController`]).
    pub can_ring: bool,
    /// The left/right volume balance can be adjusted (see [`DeviceState::balance`]).
    pub has_balance: bool,
    /// Gestures whose action can be changed; `None` when there are none.
    pub controls: Option<ControlCapabilities>,
    /// The noise-control modes a [`ControlAction::NoiseControl`] gesture can
    /// cycle through, in display order; empty when the cycle is fixed. See
    /// [`DeviceState::anc_cycle`].
    pub anc_cycle_modes: Vec<AncMode>,
}

/// The fewest modes a noise-control cycle may hold (as the vendor apps require).
pub const ANC_CYCLE_MIN_MODES: u32 = 2;

/// The gesture controls a model lets the user remap.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ControlCapabilities {
    /// How the buds are operated, so the UI can name gestures the vendor's
    /// way ("Double pinch", "Double tap").
    pub input: ControlInput,
    /// Every remappable gesture, in display order.
    pub slots: Vec<ControlSlot>,
}

/// One remappable gesture on one bud and the actions it can take.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ControlSlot {
    pub side: BudSide,
    pub gesture: ControlGesture,
    /// The actions this gesture can be set to, in display order.
    pub actions: Vec<ControlAction>,
}

/// A gesture's current action, as the device reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ControlSetting {
    pub side: BudSide,
    pub gesture: ControlGesture,
    pub action: ControlAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum BudSide {
    Left,
    Right,
}

/// How the buds are operated. (The UI owns the wording.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ControlInput {
    /// Squeezing the stem (e.g. Nothing).
    Pinch,
    /// Tapping a touch surface.
    Tap,
    /// Pressing a physical button.
    Press,
}

/// A vendor-neutral gesture, named in the UI after [`ControlInput`]. (Not
/// called `Gesture`, which would clash with SwiftUI's `Gesture`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ControlGesture {
    Single,
    Double,
    Triple,
    Hold,
    DoubleHold,
}

/// What a gesture does. (The UI owns the titles.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ControlAction {
    PlayPause,
    SkipForward,
    SkipBack,
    VolumeUp,
    VolumeDown,
    VoiceAssistant,
    NoiseControl,
    /// The gesture does nothing.
    NoAction,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqCapabilities {
    /// Display name of each band, low to high (e.g. "60 Hz" or "Bass"). Not
    /// every vendor publishes band frequencies, so these are labels, not numbers.
    pub band_labels: Vec<String>,
    /// Gains are in this unit.
    pub unit: EqUnit,
    pub min_gain: f32,
    pub max_gain: f32,
    pub step: f32,
    /// The model's presets, in display order; empty when it has none. A
    /// custom curve is available whenever `band_labels` is non-empty.
    pub presets: Vec<EqPreset>,
}

/// What an EQ gain value means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum EqUnit {
    Decibels,
    /// Device-defined steps with no published dB value (e.g. bass level -1…4).
    Level,
}

/// Last known state reported by the device. Fields stay `None` until the device
/// has reported them.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct DeviceState {
    pub battery: BatteryStatus,
    pub anc: Option<AncMode>,
    /// Whether `anc` can be changed from BarBuds right now.
    pub anc_availability: AncAvailability,
    pub eq: Option<EqSettings>,
    /// The confirmed value of each supported toggle the device has reported.
    pub toggles: HashMap<FeatureToggle, bool>,
    /// Firmware versions and serial numbers, once the device has reported them.
    pub info: DeviceInfo,
    /// Left/right volume balance from [`BALANCE_MIN`] (only the left bud plays)
    /// through 0 (centred) to [`BALANCE_MAX`] (only the right bud plays).
    pub balance: Option<i32>,
    /// The current action of each remappable gesture the device has reported.
    pub controls: Vec<ControlSetting>,
    /// The modes a noise-control gesture cycles through, in
    /// [`DeviceCapabilities::anc_cycle_modes`] order, once reported.
    pub anc_cycle: Option<Vec<AncMode>>,
}

/// See [`DeviceState::balance`].
pub const BALANCE_MIN: i32 = -100;
/// See [`DeviceState::balance`].
pub const BALANCE_MAX: i32 = 100;

/// One string per component, e.g. a firmware version. `None` when the
/// component hasn't reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ComponentStrings {
    pub left: Option<String>,
    pub right: Option<String>,
    pub charging_case: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    /// As the vendor's app displays it, e.g. "release_5.11_signed" or "3.581.0".
    pub firmware: ComponentStrings,
    pub serial_numbers: ComponentStrings,
}
