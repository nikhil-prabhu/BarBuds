import BarBudsCore
import SwiftUI

struct EqControl: View {
    let capabilities: EqCapabilities
    let settings: EqSettings
    let onSelectPreset: (EqPreset) -> Void
    /// Moving any slider switches the device to the Custom preset.
    let onSetGain: (_ gainDb: Float, _ bandIndex: Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Equalizer")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                Picker("Preset", selection: presetBinding) {
                    ForEach(capabilities.presets, id: \.self) { preset in
                        Text(preset.title).tag(preset)
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .fixedSize()
            }

            VStack(spacing: 2) {
                ForEach(Array(settings.gainsDb.enumerated()), id: \.offset) { index, gainDb in
                    bandRow(gainDb, index: index)
                }
            }
        }
    }

    private var presetBinding: Binding<EqPreset> {
        Binding(get: { settings.preset }, set: onSelectPreset)
    }

    private func bandRow(_ gainDb: Float, index: Int) -> some View {
        let label = capabilities.bandLabels.indices.contains(index)
            ? capabilities.bandLabels[index]
            : "Band \(index + 1)"
        return HStack(spacing: 8) {
            Text(label)
                .lineLimit(1)
                .frame(width: 76, alignment: .leading)
            Slider(
                value: Binding(get: { gainDb }, set: { onSetGain($0, index) }),
                in: capabilities.minGainDb...capabilities.maxGainDb,
                step: capabilities.stepDb
            )
            .controlSize(.small)
            .accessibilityLabel(label)
            .accessibilityValue(Units.gain(gainDb))
            Text(Units.gain(gainDb))
                .frame(width: 44, alignment: .trailing)
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(.secondary)
    }
}

#Preview {
    EqControl(
        capabilities: EqCapabilities(
            bandLabels: ["Low Bass", "Bass", "Mid", "Treble", "Upper Treble"],
            minGainDb: -6,
            maxGainDb: 6,
            stepDb: 0.5,
            presets: [.flat, .bassBoost, .vocal, .trebleBoost, .custom]
        ),
        settings: EqSettings(preset: .custom, gainsDb: [1, 0.5, 0, -1.5, 2]),
        onSelectPreset: { _ in },
        onSetGain: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
