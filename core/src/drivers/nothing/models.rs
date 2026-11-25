//! What each Nothing and CMF model supports, by the model ID the earbuds
//! report (`0xC01C`). Taken from BudsLink's per-model configurations
//! (GPL-3.0, <https://github.com/maniacx/BudsLink>; facts only, no code),
//! limited to what BarBuds models: battery, noise control (Off, Transparency
//! and Noise Cancelling, whose strengths are kept as the buds report them),
//! EQ presets, in-ear detection, Dual Connection, Low Latency and gestures on
//! the buds themselves (not headphone dials, sliders or case knobs).
//!
//! Only the Ear (open) is confirmed on hardware; the rest are expected to
//! work but untested. BudsLink's tables are known to be incomplete: they don't
//! list Dual Connection for the Ear (open), which it has.

use crate::model::{BudSide, ControlAction, ControlGesture, FormFactor};

/// One model's features.
#[derive(Debug)]
pub(crate) struct NothingModel {
    /// As reported, e.g. `0xB174` for the Ear (open).
    pub id: u16,
    pub name: &'static str,
    /// The end of the factory Bluetooth name (BudsLink's patterns), for
    /// recognising the model without asking it.
    pub bluetooth_name: &'static str,
    pub form_factor: FormFactor,
    /// One battery (headphones, neckband), reported as the left bud's.
    pub single_battery: bool,
    pub anc: Option<AncCodes>,
    pub presets: &'static [Preset],
    /// Presets are set with the listening-mode command (`0xF01D`) instead of
    /// the EQ command (`0xF010`), as on CMF earbuds. They're read the same way.
    pub listening_mode: bool,
    /// The 3-band Custom curve is shown as sliders (code [`super::CUSTOM_CODE`]).
    pub custom_sliders: bool,
    pub in_ear_detection: bool,
    pub dual_connection: bool,
    pub gestures: &'static [Slot],
    /// Confirmed on real hardware.
    pub tested: bool,
}

/// Noise-control mode codes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AncCodes {
    pub off: u8,
    pub transparency: Option<u8>,
    /// Every Noise Cancelling strength; the first is set when the buds
    /// haven't reported one yet.
    pub cancelling: &'static [u8],
}

/// `(id, name as the vendor's app shows it, wire code)`.
pub(crate) type Preset = (&'static str, &'static str, u8);

/// A remappable gesture on one bud and the actions it can be set to.
#[derive(Debug)]
pub(crate) struct Slot {
    pub side: BudSide,
    pub gesture: ControlGesture,
    pub actions: &'static [ControlAction],
}

/// Noise Cancelling strengths: high, mid, low, adaptive.
const ALL_STRENGTHS: AncCodes = AncCodes {
    off: 0x05,
    transparency: Some(0x07),
    cancelling: &[0x01, 0x02, 0x03, 0x04],
};

/// One Noise Cancelling code, as on the CMF Buds, Buds 2a and Buds Neo.
const ONE_STRENGTH: AncCodes = AncCodes {
    off: 0x05,
    transparency: Some(0x07),
    cancelling: &[0x04],
};

/// Nothing-branded presets. Confirmed on the Ear (open), whose app names
/// them this way.
const NOTHING_PRESETS: &[Preset] = &[
    ("balanced", "Balanced", 0x00),
    ("more-bass", "More Bass", 0x03),
    ("more-treble", "More Treble", 0x02),
    ("voice", "Voice", 0x01),
];

/// Nothing presets plus the user's own curve, which the Nothing X app edits.
const NOTHING_WITH_CUSTOM: &[Preset] = &[
    ("balanced", "Balanced", 0x00),
    ("more-bass", "More Bass", 0x03),
    ("more-treble", "More Treble", 0x02),
    ("voice", "Voice", 0x01),
    ("custom", "Custom", 0x05),
];

/// Plus the Nothing X app's Advanced EQ curve.
const NOTHING_WITH_ADVANCED: &[Preset] = &[
    ("balanced", "Balanced", 0x00),
    ("more-bass", "More Bass", 0x03),
    ("more-treble", "More Treble", 0x02),
    ("voice", "Voice", 0x01),
    ("custom", "Custom", 0x05),
    ("advanced", "Advanced EQ", 0x06),
];

/// CMF presets.
const CMF_PRESETS: &[Preset] = &[
    ("dirac", "Dirac", 0x00),
    ("pop", "Pop", 0x03),
    ("rock", "Rock", 0x01),
    ("electronic", "Electronic", 0x02),
    ("enhance-vocals", "Enhance Vocals", 0x04),
    ("classical", "Classical", 0x05),
    ("custom", "Custom", 0x06),
];

/// The CMF Buds 2a's Dirac preset has its own code.
const CMF_2A_PRESETS: &[Preset] = &[
    ("dirac", "Dirac", 0x07),
    ("pop", "Pop", 0x03),
    ("rock", "Rock", 0x01),
    ("electronic", "Electronic", 0x02),
    ("enhance-vocals", "Enhance Vocals", 0x04),
    ("classical", "Classical", 0x05),
    ("custom", "Custom", 0x06),
];

/// The CMF Buds 2 Plus have no Dirac preset.
const CMF_2_PLUS_PRESETS: &[Preset] = &[
    ("pop", "Pop", 0x03),
    ("rock", "Rock", 0x01),
    ("electronic", "Electronic", 0x02),
    ("enhance-vocals", "Enhance Vocals", 0x04),
    ("classical", "Classical", 0x05),
    ("custom", "Custom", 0x06),
];

use ControlAction::{
    NoAction, NoiseControl, PlayPause, SkipBack, SkipForward, VoiceAssistant, VolumeDown, VolumeUp,
};
use ControlGesture::{Double, DoubleHold, Hold, Triple};

/// The same slot on both buds.
macro_rules! both {
    ($($gesture:expr => $actions:expr),* $(,)?) => {
        &[
            $(Slot { side: BudSide::Left, gesture: $gesture, actions: $actions },)*
            $(Slot { side: BudSide::Right, gesture: $gesture, actions: $actions },)*
        ]
    };
}

/// CMF Buds Pro.
const CMF_PRO_GESTURES: &[Slot] = both![
    Double => &[PlayPause, SkipBack, SkipForward, VoiceAssistant],
    Triple => &[SkipBack, SkipForward, VoiceAssistant],
    Hold => &[NoiseControl, VoiceAssistant],
    DoubleHold => &[VolumeUp, VolumeDown, VoiceAssistant, NoAction],
];

/// Later CMF earbuds.
const CMF_GESTURES: &[Slot] = both![
    Double => &[PlayPause, SkipBack, SkipForward, VoiceAssistant, NoAction],
    Triple => &[SkipBack, SkipForward, VoiceAssistant, NoAction],
    Hold => &[NoiseControl, VoiceAssistant, NoAction],
    DoubleHold => &[VolumeUp, VolumeDown, VoiceAssistant, NoAction],
];

/// Nothing Ear, Ear (a) and Ear (stick).
const NOTHING_GESTURES: &[Slot] = both![
    Double => &[SkipBack, SkipForward, VoiceAssistant],
    Triple => &[SkipBack, SkipForward, VoiceAssistant],
    Hold => &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant],
    DoubleHold => &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant, NoAction],
];

/// The Ear (2) lists only the left bud's gestures.
const EAR_2_GESTURES: &[Slot] = &[
    Slot {
        side: BudSide::Left,
        gesture: Double,
        actions: &[SkipBack, SkipForward, VoiceAssistant],
    },
    Slot {
        side: BudSide::Left,
        gesture: Triple,
        actions: &[SkipBack, SkipForward, VoiceAssistant],
    },
    Slot {
        side: BudSide::Left,
        gesture: Hold,
        actions: &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant],
    },
    Slot {
        side: BudSide::Left,
        gesture: DoubleHold,
        actions: &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant, NoAction],
    },
];

const EAR_1_GESTURES: &[Slot] = both![
    Triple => &[SkipBack, SkipForward, VoiceAssistant, NoAction],
    Hold => &[NoiseControl, NoAction],
];

/// Ear (3) and Ear (3a), without their microphone and recording actions.
const EAR_3_GESTURES: &[Slot] = both![
    Double => &[SkipForward, SkipBack, VoiceAssistant],
    Triple => &[SkipForward, SkipBack, VoiceAssistant, NoAction],
    Hold => &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant, NoAction],
    DoubleHold => &[NoiseControl, VolumeUp, VolumeDown, VoiceAssistant, NoAction],
];

const EAR_OPEN_GESTURES: &[Slot] = both![
    Double => &[SkipBack, SkipForward, VoiceAssistant],
    Triple => &[SkipBack, SkipForward, VoiceAssistant],
    Hold => &[VolumeUp, VolumeDown, VoiceAssistant],
    DoubleHold => &[VolumeUp, VolumeDown, VoiceAssistant, NoAction],
];

/// Every model BarBuds knows, in the README's order.
pub(crate) const MODELS: &[NothingModel] = &[
    NothingModel {
        id: 0xB181,
        bluetooth_name: "Nothing ear (1)",
        name: "Nothing Ear (1)",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(AncCodes {
            off: 0x05,
            transparency: Some(0x07),
            cancelling: &[0x01, 0x03],
        }),
        presets: NOTHING_PRESETS,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: false,
        gestures: EAR_1_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB155,
        bluetooth_name: "Ear (2)",
        name: "Nothing Ear (2)",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: EAR_2_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB157,
        bluetooth_name: "Ear (stick)",
        name: "Nothing Ear (stick)",
        form_factor: FormFactor::Stemmed,
        single_battery: false,
        // No ANC hardware; BudsLink's codes for it (Off = 1) would mean
        // "ANC high" on every other model.
        anc: None,
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: false,
        gestures: NOTHING_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB171,
        bluetooth_name: "Nothing Ear",
        name: "Nothing Ear",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: NOTHING_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB183,
        bluetooth_name: "Nothing Ear (a)",
        name: "Nothing Ear (a)",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_CUSTOM,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: NOTHING_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB173,
        bluetooth_name: "Nothing Ear (3)",
        name: "Nothing Ear (3)",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: EAR_3_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB190,
        bluetooth_name: "Nothing Ear (3a)",
        name: "Nothing Ear (3a)",
        form_factor: FormFactor::StemmedInEar,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: EAR_3_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB174,
        bluetooth_name: "Nothing Ear (open)",
        name: "Nothing Ear (open)",
        form_factor: FormFactor::EarHook,
        single_battery: false,
        anc: None,
        // The Custom curve is the sliders.
        presets: NOTHING_PRESETS,
        listening_mode: false,
        custom_sliders: true,
        in_ear_detection: false,
        dual_connection: true,
        gestures: EAR_OPEN_GESTURES,
        tested: true,
    },
    NothingModel {
        id: 0xB170,
        bluetooth_name: "Nothing Headphone (1)",
        name: "Nothing Headphone (1)",
        form_factor: FormFactor::Headphones,
        single_battery: true,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: &[],
        tested: false,
    },
    NothingModel {
        id: 0xB198,
        bluetooth_name: "Headphone (a)",
        name: "Nothing Headphone (a)",
        form_factor: FormFactor::Headphones,
        single_battery: true,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_ADVANCED,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: true,
        gestures: &[],
        tested: false,
    },
    NothingModel {
        id: 0xB163,
        bluetooth_name: "Buds Pro",
        name: "CMF Buds Pro",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(AncCodes {
            off: 0x05,
            transparency: Some(0x07),
            cancelling: &[0x01, 0x02, 0x03],
        }),
        presets: NOTHING_WITH_CUSTOM,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: false,
        gestures: CMF_PRO_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB187,
        bluetooth_name: "CMF Buds Pro 2",
        name: "CMF Buds Pro 2",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: CMF_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB168,
        bluetooth_name: "CMF Buds",
        name: "CMF Buds",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ONE_STRENGTH),
        presets: CMF_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB179,
        bluetooth_name: "CMF Buds 2",
        name: "CMF Buds 2",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: CMF_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB185,
        bluetooth_name: "CMF Buds 2a",
        name: "CMF Buds 2a",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ONE_STRENGTH),
        presets: CMF_2A_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB184,
        bluetooth_name: "CMF Buds 2 Plus",
        name: "CMF Buds 2 Plus",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ALL_STRENGTHS),
        presets: CMF_2_PLUS_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB193,
        bluetooth_name: "CMF Buds Neo",
        name: "CMF Buds Neo",
        form_factor: FormFactor::Stemless,
        single_battery: false,
        anc: Some(ONE_STRENGTH),
        presets: CMF_PRESETS,
        listening_mode: true,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: true,
        gestures: CMF_GESTURES,
        tested: false,
    },
    NothingModel {
        id: 0xB175,
        bluetooth_name: "CMF Headphone Pro",
        name: "CMF Headphone Pro",
        form_factor: FormFactor::Headphones,
        single_battery: true,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_CUSTOM,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: true,
        dual_connection: true,
        gestures: &[],
        tested: false,
    },
    NothingModel {
        id: 0xB164,
        bluetooth_name: "Neckband Pro",
        name: "CMF Neckband Pro",
        form_factor: FormFactor::Neckband,
        single_battery: true,
        anc: Some(ALL_STRENGTHS),
        presets: NOTHING_WITH_CUSTOM,
        listening_mode: false,
        custom_sliders: false,
        in_ear_detection: false,
        dual_connection: true,
        gestures: &[],
        tested: false,
    },
];

/// The model a Bluetooth name belongs to: the longest factory name it ends
/// with, ignoring case (so "CMF Buds Pro 2" isn't taken for "Buds Pro").
pub(crate) fn model_for_name(name: &str) -> Option<&'static NothingModel> {
    let name = name.trim().to_lowercase();
    MODELS
        .iter()
        .filter(|m| name.ends_with(&m.bluetooth_name.to_lowercase()))
        .max_by_key(|m| m.bluetooth_name.len())
}

/// The model with this ID, if BarBuds knows it.
pub(crate) fn model(id: u16) -> Option<&'static NothingModel> {
    MODELS.iter().find(|m| m.id == id)
}
