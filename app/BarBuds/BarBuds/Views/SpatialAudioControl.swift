import BarBudsCore
import SwiftUI

/// Spatial audio modes as buttons, like noise control: Off and Fixed on
/// earbuds, plus Head Tracking or Concert and Cinema on some headphones.
struct SpatialAudioControl: View {
    let modes: [SpatialAudioMode]
    /// `nil` until the device reports its mode.
    let selected: SpatialAudioMode?
    let onSelect: (SpatialAudioMode) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle("Spatial Audio", detail: selected?.title)
            HStack(spacing: 8) {
                ForEach(modes, id: \.self) { mode in
                    modeButton(mode)
                }
            }
            .disabled(selected == nil)
            if let selected {
                Text(selected.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func modeButton(_ mode: SpatialAudioMode) -> some View {
        let isSelected = mode == selected
        let button = Button {
            onSelect(mode)
        } label: {
            VStack(spacing: 4) {
                Image(systemName: mode.systemImage)
                    .font(.title3)
                    .frame(height: 22)
                Text(mode.title)
                    .font(.caption)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
        }
        .help(mode.detail)
        .accessibilityLabel(mode.title)
        .accessibilityAddTraits(isSelected ? .isSelected : [])

        if isSelected {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.glass)
        }
    }
}

#Preview("Head tracking") {
    SpatialAudioControl(modes: [.off, .fixed, .headTracking], selected: .headTracking, onSelect: { _ in })
        .padding()
        .frame(width: 300)
}

#Preview("Earbuds") {
    SpatialAudioControl(modes: [.off, .fixed], selected: .off, onSelect: { _ in })
        .padding()
        .frame(width: 300)
}
