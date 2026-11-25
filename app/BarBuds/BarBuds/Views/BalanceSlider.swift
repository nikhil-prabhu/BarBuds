import SwiftUI

/// Left/right volume balance: -100 (only the left bud plays) … 0 (centred) …
/// 100 (only the right bud plays), matching the core's `DeviceState.balance`.
/// A collapsible section whose header shows the value while collapsed.
struct BalanceSlider: View {
    /// `nil` until the device reports it; the slider is disabled until then.
    let value: Int32?
    @Binding var isExpanded: Bool
    let onChange: (Int32) -> Void

    var body: some View {
        CollapsibleSection(title: "Balance", summary: Self.label(for: value), isExpanded: $isExpanded) {
            VStack(spacing: 2) {
                HStack(spacing: 8) {
                    Text("L")
                    Slider(
                        value: Binding(
                            get: { Double(value ?? 0) },
                            set: { onChange(Int32($0.rounded())) }
                        ),
                        in: -100...100,
                        step: 5
                    )
                    .controlSize(.small)
                    .accessibilityLabel("Balance")
                    .accessibilityValue(Self.label(for: value))
                    Text("R")
                }
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)

                Text(Self.label(for: value))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            .disabled(value == nil)
        }
    }

    /// "Centered", "Left 30" (the left side is favoured), or "Right 30".
    static func label(for value: Int32?) -> String {
        guard let value else { return "–" }
        switch value {
        case 0: return "Centered"
        case ..<0: return "Left \(-value)"
        default: return "Right \(value)"
        }
    }
}

#Preview {
    VStack(spacing: 16) {
        BalanceSlider(value: -30, isExpanded: .constant(true), onChange: { _ in })
        BalanceSlider(value: 0, isExpanded: .constant(false), onChange: { _ in })
        BalanceSlider(value: nil, isExpanded: .constant(true), onChange: { _ in })
    }
    .padding()
    .frame(width: 300)
}
