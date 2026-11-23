import BarBudsCore
import SwiftUI

struct EqControl: View {
    let capabilities: EqCapabilities
    let settings: EqSettings
    let onSelectPreset: (EqPreset) -> Void
    /// Moving any slider switches the device to the Custom preset.
    let onSetGain: (_ gain: Float, _ bandIndex: Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Equalizer")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                // Models with only a Custom curve have nothing to pick.
                if capabilities.presets.count > 1 {
                    Picker("Preset", selection: presetBinding) {
                        ForEach(capabilities.presets, id: \.self) { preset in
                            Text(preset.title).tag(preset)
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

    private var presetBinding: Binding<EqPreset> {
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
            presets: [.flat, .bassBoost, .vocal, .trebleBoost, .custom]
        ),
        settings: EqSettings(preset: .custom, gains: [1, 0.5, 0, -1.5, 2]),
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
            presets: [.custom]
        ),
        settings: EqSettings(preset: .custom, gains: [2]),
        onSelectPreset: { _ in },
        onSetGain: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
