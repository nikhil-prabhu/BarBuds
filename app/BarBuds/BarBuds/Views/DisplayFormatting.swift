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
        }
    }
}

extension EqPreset {
    var title: String {
        switch self {
        case .flat: "Flat"
        case .bassBoost: "Bass Boost"
        case .vocal: "Vocal"
        case .trebleBoost: "Treble Boost"
        case .custom: "Custom"
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
