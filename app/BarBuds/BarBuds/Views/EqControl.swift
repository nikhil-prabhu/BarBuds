import BarBudsCore
import SwiftUI

struct EqControl: View {
    let capabilities: EqCapabilities
    let settings: EqSettings
    /// Selects a preset by ID; `nil` switches to a custom curve.
    let onSelectPreset: (String?) -> Void
    /// Moving any slider switches the device to a custom curve.
    let onSetGain: (_ gain: Float, _ bandIndex: Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Equalizer")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                // Models with only a custom curve have nothing to pick.
                if !capabilities.presets.isEmpty {
                    Picker("Preset", selection: presetBinding) {
                        ForEach(capabilities.presets, id: \.id) { preset in
                            Text(preset.name).tag(Optional(preset.id))
                        }
                        // Presets are the vendor's; "Custom" means the user's own curve.
                        if !capabilities.bandLabels.isEmpty {
                            Divider()
                            Text("Custom").tag(String?.none)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .fixedSize()
                }
            }

            VStack(spacing: 2) {
                ForEach(Array(settings.gains.enumerated()), id: \.offset) { index, gain in
                    bandRow(gain, index: index)
                }
            }
        }
    }

    private var presetBinding: Binding<String?> {
        Binding(get: { settings.preset }, set: onSelectPreset)
    }

    private func bandRow(_ gain: Float, index: Int) -> some View {
        let label = capabilities.bandLabels.indices.contains(index)
            ? capabilities.bandLabels[index]
            : "Band \(index + 1)"
        return HStack(spacing: 8) {
            Text(label)
                .lineLimit(1)
                .frame(width: 76, alignment: .leading)
            Slider(
                value: Binding(get: { gain }, set: { onSetGain($0, index) }),
                in: capabilities.minGain...capabilities.maxGain,
                step: capabilities.step
            )
            .controlSize(.small)
            .accessibilityLabel(label)
            .accessibilityValue(Units.gain(gain, unit: capabilities.unit))
            Text(Units.gain(gain, unit: capabilities.unit))
                .frame(width: 44, alignment: .trailing)
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(.secondary)
    }
}

#Preview("Five bands") {
    EqControl(
        capabilities: EqCapabilities(
            bandLabels: ["Low Bass", "Bass", "Mid", "Treble", "Upper Treble"],
            unit: .decibels,
            minGain: -6,
            maxGain: 6,
            step: 0.5,
            presets: [
                EqPreset(id: "default", name: "Default"),
                EqPreset(id: "heavy-bass", name: "Heavy Bass"),
                EqPreset(id: "vocal-boost", name: "Vocal Boost"),
            ]
        ),
        settings: EqSettings(preset: nil, gains: [1, 0.5, 0, -1.5, 2]),
        onSelectPreset: { _ in },
        onSetGain: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}

#Preview("Bass only") {
    EqControl(
        capabilities: EqCapabilities(
            bandLabels: ["Bass"],
            unit: .level,
            minGain: -1,
            maxGain: 4,
            step: 1,
            presets: []
        ),
        settings: EqSettings(preset: nil, gains: [2]),
        onSelectPreset: { _ in },
        onSetGain: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
