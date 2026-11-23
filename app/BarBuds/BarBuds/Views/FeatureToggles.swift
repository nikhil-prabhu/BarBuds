import BarBudsCore
import SwiftUI

/// On/off device features, e.g. in-ear detection.
struct FeatureToggles: View {
    let toggles: [FeatureToggle]
    /// Confirmed values. A toggle the device hasn't reported yet is disabled.
    let values: [FeatureToggle: Bool]
    let onSet: (FeatureToggle, Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle("Settings")
            ForEach(toggles, id: \.self) { toggle in
                row(toggle)
            }
        }
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

#Preview {
    FeatureToggles(
        toggles: [.adaptiveSound, .volumeEq, .inEarDetection],
        values: [.adaptiveSound: false, .volumeEq: true, .inEarDetection: true],
        onSet: { _, _ in }
    )
    .padding()
    .frame(width: 300)
}
