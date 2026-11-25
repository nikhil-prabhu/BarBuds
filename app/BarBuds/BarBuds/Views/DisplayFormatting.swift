import BarBudsCore
import Foundation

extension AncMode {
    var title: String {
        switch self {
        case .off: "Off"
        case .noiseCancelling: "Noise Cancelling"
        case .transparency: "Transparency"
        }
    }

    /// Short label for compact controls.
    var shortTitle: String {
        switch self {
        case .off: "Off"
        case .noiseCancelling: "ANC"
        case .transparency: "Ambient"
        }
    }

    var systemImage: String {
        switch self {
        case .off: "circle.slash"
        case .noiseCancelling: "waveform.slash"
        case .transparency: "ear"
        }
    }
}

/// Generic, unbranded SF Symbols for each shape. (Product symbols such as
/// `airpods` may only be used for Apple's own products.)
extension FormFactor {
    var pairSymbol: String {
        switch self {
        case .stemmed: "earbuds"
        case .stemmedInEar: "earbuds.in.ear"
        case .stemless: "earbuds.stemless"
        case .earHook: "earbuds.bone.conduction"
        }
    }

    var leftSymbol: String {
        switch self {
        case .stemmed: "earbud.left"
        default: pairSymbol + ".left"
        }
    }

    var rightSymbol: String {
        switch self {
        case .stemmed: "earbud.right"
        default: pairSymbol + ".right"
        }
    }

    static let caseSymbol = "earbuds.case.fill"
}

extension FeatureToggle {
    var title: String {
        switch self {
        case .inEarDetection: "In-Ear Detection"
        case .adaptiveSound: "Adaptive Sound"
        case .volumeEq: "Volume EQ"
        case .conversationDetection: "Conversation Detection"
        case .multipoint: "Multipoint"
        case .monoAudio: "Mono Audio"
        case .touchControls: "Touch Controls"
        case .lowLatency: "Low Latency Mode"
        }
    }

    var detail: String {
        switch self {
        case .inEarDetection: "Pause when you take an earbud out"
        case .adaptiveSound: "Raise volume in noisy places"
        case .volumeEq: "Keep bass and treble full at low volume"
        case .conversationDetection: "Pause audio when you start talking. Works only with both earbuds in."
        case .multipoint: "Stay connected to two devices at once"
        case .monoAudio: "Play the same audio in both earbuds"
        case .touchControls: "Tap and swipe on the earbuds"
        case .lowLatency: "Reduce audio delay for games and video"
        }
    }
}

extension ControlGesture {
    /// E.g. "Double Pinch" or "Tap & Hold", in the buds' own terms.
    func title(for input: ControlInput) -> String {
        let verb = switch input {
        case .pinch: "Pinch"
        case .tap: "Tap"
        case .press: "Press"
        }
        return switch self {
        case .single: verb
        case .double: "Double \(verb)"
        case .triple: "Triple \(verb)"
        case .hold: "\(verb) & Hold"
        case .doubleHold: "Double \(verb) & Hold"
        }
    }
}

extension ControlAction {
    var title: String {
        switch self {
        case .playPause: "Play/Pause"
        case .skipForward: "Skip Forward"
        case .skipBack: "Skip Back"
        case .volumeUp: "Volume Up"
        case .volumeDown: "Volume Down"
        case .voiceAssistant: "Voice Assistant"
        case .noiseControl: "Noise Control"
        case .noAction: "No Action"
        }
    }
}

extension BudSide {
    var title: String {
        switch self {
        case .left: "Left"
        case .right: "Right"
        }
    }
}

enum Units {
    /// Decibels: 3 → "+3 dB", 0 → "0 dB", -2.5 → "-2.5 dB". Levels: 2 → "+2".
    static func gain(_ value: Float, unit: EqUnit) -> String {
        let number = value.formatted(
            .number
                .sign(strategy: .always(includingZero: false))
                .precision(.fractionLength(0...1))
        )
        switch unit {
        case .decibels: return "\(number) dB"
        case .level: return number
        }
    }
}
