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
                ForEach(Array(settings.bands.enumerated()), id: \.offset) { index, band in
                    bandRow(band, index: index)
                }
            }
        }
    }

    private var presetBinding: Binding<EqPreset> {
        Binding(get: { settings.preset }, set: onSelectPreset)
    }

    private func bandRow(_ band: EqBand, index: Int) -> some View {
        HStack(spacing: 8) {
            Text(Units.frequency(band.frequencyHz))
                .frame(width: 48, alignment: .leading)
            Slider(
                value: Binding(get: { band.gainDb }, set: { onSetGain($0, index) }),
                in: capabilities.minGainDb...capabilities.maxGainDb,
                step: capabilities.stepDb
            )
            .controlSize(.small)
            .accessibilityLabel(Units.frequency(band.frequencyHz))
            .accessibilityValue(Units.gain(band.gainDb))
            Text(Units.gain(band.gainDb))
                .frame(width: 44, alignment: .trailing)
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(.secondary)
    }
}

#Preview {
    EqControl(
        capabilities: EqCapabilities(
            bandFrequenciesHz: [60, 250, 1_000, 4_000, 12_000],
            minGainDb: -6,
            maxGainDb: 6,
            stepDb: 1,
            presets: [.flat, .bassBoost, .vocal, .trebleBoost, .custom]
        ),
        settings: EqSettings(
            preset: .bassBoost,
            bands: zip([60, 250, 1_000, 4_000, 12_000], [5, 3, 0, 0, 0] as [Float])
                .map { EqBand(frequencyHz: $0, gainDb: $1) }
        ),
        onSelectPreset: { _ in },
        onSetGain: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
