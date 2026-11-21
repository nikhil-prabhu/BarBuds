import BarBudsCore
import SwiftUI

struct AncControl: View {
    let modes: [AncMode]
    /// `nil` until the device reports its mode.
    let selected: AncMode?
    let onSelect: (AncMode) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle("Noise Control", detail: selected?.title)
            HStack(spacing: 8) {
                ForEach(modes, id: \.self) { mode in
                    modeButton(mode)
                }
            }
        }
    }

    @ViewBuilder
    private func modeButton(_ mode: AncMode) -> some View {
        let isSelected = mode == selected
        let button = Button {
            onSelect(mode)
        } label: {
            VStack(spacing: 4) {
                Image(systemName: mode.systemImage)
                    .font(.title3)
                    .frame(height: 22)
                Text(mode.shortTitle)
                    .font(.caption)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
        }
        .help(mode.title)
        .accessibilityLabel(mode.title)
        .accessibilityAddTraits(isSelected ? .isSelected : [])

        if isSelected {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.glass)
        }
    }
}

struct SectionTitle: View {
    let title: String
    let detail: String?

    init(_ title: String, detail: String? = nil) {
        self.title = title
        self.detail = detail
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.subheadline.weight(.semibold))
            Spacer()
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

#Preview {
    AncControl(
        modes: [.off, .noiseCancelling, .transparency],
        selected: .noiseCancelling,
        onSelect: { _ in }
    )
    .padding()
    .frame(width: 300)
}
