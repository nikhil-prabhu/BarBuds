import BarBudsCore
import SwiftUI

/// On/off device features, e.g. in-ear detection, in a section that can be
/// collapsed to keep the popover short.
struct FeatureToggles: View {
    let toggles: [FeatureToggle]
    /// Confirmed values. A toggle the device hasn't reported yet is disabled.
    let values: [FeatureToggle: Bool]
    @Binding var isExpanded: Bool
    let onSet: (FeatureToggle, Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if isExpanded {
                ForEach(toggles, id: \.self) { toggle in
                    row(toggle)
                }
            }
        }
    }

    private var header: some View {
        Button {
            isExpanded.toggle()
        } label: {
            HStack {
                Text("Settings")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                if !isExpanded {
                    Text(summary)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Settings")
        .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
        .accessibilityHint("Shows or hides \(summary)")
    }

    /// E.g. "3 of 6 on".
    private var summary: String {
        let on = toggles.filter { values[$0] == true }.count
        return "\(on) of \(toggles.count) on"
    }

    private func row(_ toggle: FeatureToggle) -> some View {
        Toggle(isOn: Binding(
            get: { values[toggle] ?? false },
            set: { onSet(toggle, $0) }
        )) {
            VStack(alignment: .leading, spacing: 1) {
                Text(toggle.title)
                Text(toggle.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
        .controlSize(.small)
        .disabled(values[toggle] == nil)
    }
}

#Preview("Expanded") {
    FeatureToggles(
        toggles: [.conversationDetection, .volumeEq, .multipoint, .inEarDetection, .touchControls, .monoAudio],
        values: [
            .conversationDetection: false, .volumeEq: true, .multipoint: true,
            .inEarDetection: true, .touchControls: true, .monoAudio: false,
        ],
        isExpanded: .constant(true),
        onSet: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}

#Preview("Collapsed") {
    FeatureToggles(
        toggles: [.adaptiveSound, .volumeEq, .inEarDetection],
        values: [.adaptiveSound: false, .volumeEq: true, .inEarDetection: true],
        isExpanded: .constant(false),
        onSet: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
