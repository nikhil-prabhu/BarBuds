import BarBudsCore
import SwiftUI

struct AncControl: View {
    let modes: [AncMode]
    /// `nil` until the device reports its mode, and while it can't apply one
    /// (e.g. Pixel Buds report no mode while they're in the case).
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
            .disabled(selected == nil)
            if selected == nil {
                Text("Available while wearing your earbuds")
                    .font(.caption)
                    .foregroundStyle(.secondary)
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

#Preview("Worn") {
    AncControl(
        modes: [.off, .noiseCancelling, .transparency],
        selected: .noiseCancelling,
        onSelect: { _ in }
    )
    .padding()
    .frame(width: 300)
}

#Preview("In the case") {
    AncControl(
        modes: [.off, .noiseCancelling, .transparency],
        selected: nil,
        onSelect: { _ in }
    )
    .padding()
    .frame(width: 300)
}
