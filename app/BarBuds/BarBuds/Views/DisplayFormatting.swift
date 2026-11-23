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
    /// 3 → "+3 dB", 0 → "0 dB", -2.5 → "−2.5 dB".
    static func gain(_ db: Float) -> String {
        let value = db.formatted(
            .number
                .sign(strategy: .always(includingZero: false))
                .precision(.fractionLength(0...1))
        )
        return "\(value) dB"
    }
}
