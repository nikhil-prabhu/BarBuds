import BarBudsCore
import SwiftUI

struct AncControl: View {
    let modes: [AncMode]
    /// `nil` until the device reports its mode, and while it can't apply one
    /// (e.g. Pixel Buds report no mode while they're in the case).
    let selected: AncMode?
    /// Whether the mode can be changed from BarBuds right now.
    var availability: AncAvailability = .available
    /// "earbuds" or "headphones", for the hints.
    var deviceNoun = "earbuds"
    let onSelect: (AncMode) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle("Noise Control", detail: selected?.title)
            HStack(spacing: 8) {
                ForEach(modes, id: \.self) { mode in
                    modeButton(mode)
                }
            }
            if let hint {
                Text(hint)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var hint: String? {
        switch availability {
        case .onDeviceOnly:
            "These \(deviceNoun) only let you change this on the \(deviceNoun) themselves."
        case .notNow:
            "Available while wearing your \(deviceNoun)"
        case .available:
            selected == nil ? "Available while wearing your \(deviceNoun)" : nil
        }
    }

    /// The mode can't be changed from BarBuds right now.
    private var isLocked: Bool {
        selected == nil || availability != .available
    }

    @ViewBuilder
    private func modeButton(_ mode: AncMode) -> some View {
        let isSelected = mode == selected
        let button = Button {
            guard !isLocked else { return }
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

        if isSelected, isLocked {
            // Keep the active mode's colour, dimmed, so it stays obvious
            // which mode is on (disabling it would turn it grey).
            button.buttonStyle(.glassProminent)
                .opacity(0.5)
                .allowsHitTesting(false)
                .accessibilityAddTraits(.isStaticText)
        } else if isSelected {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.glass)
                .disabled(isLocked)
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

#Preview("On the earbuds only") {
    AncControl(
        modes: [.off, .noiseCancelling, .transparency],
        selected: .noiseCancelling,
        availability: .onDeviceOnly,
        onSelect: { _ in }
    )
    .padding()
    .frame(width: 300)
}
