import BarBudsCore
import SwiftUI

/// Rings each bud so the user can find it, like Android's "Find device"
/// panel: a button per bud, which turns into "Mute" while that bud rings.
struct FindControl: View {
    let find: FindSession
    let formFactor: FormFactor
    @Binding var isExpanded: Bool

    var body: some View {
        // Always open while a bud rings, so Mute is never hidden.
        CollapsibleSection(
            title: "Find \(formFactor.deviceTitle)",
            isExpanded: Binding(get: { isExpanded || find.isRinging }, set: { isExpanded = $0 })
        ) {
            controls
        }
    }

    @ViewBuilder
    private var controls: some View {
        HStack(spacing: 8) {
            if formFactor.isSingleUnit {
                // One piece: Fast Pair rings it with the bit used for the
                // right bud, so either bud's ringing counts.
                budButton(
                    formFactor.deviceTitle,
                    systemImage: formFactor.pairSymbol,
                    isRinging: find.isRinging,
                    accessibilityName: formFactor.deviceNoun
                ) {
                    ring(left: false, right: !find.isRinging)
                }
            } else {
                budButton(
                    "Left",
                    systemImage: formFactor.leftSymbol,
                    isRinging: find.ringing.left,
                    accessibilityName: "left earbud"
                ) {
                    ring(left: !find.ringing.left, right: find.ringing.right)
                }
                budButton(
                    "Right",
                    systemImage: formFactor.rightSymbol,
                    isRinging: find.ringing.right,
                    accessibilityName: "right earbud"
                ) {
                    ring(left: find.ringing.left, right: !find.ringing.right)
                }
            }
        }
        .disabled(find.connection == .opening)

        if let error = find.lastError {
            Label(error, systemImage: "exclamationmark.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            Text(formFactor == .headphones
                 ? "Take your headphones off first. The sound is loud."
                 : "Take your earbuds out first. The sound is loud.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func ring(left: Bool, right: Bool) {
        Task { await find.ring(left: left, right: right) }
    }

    @ViewBuilder
    private func budButton(
        _ side: String,
        systemImage: String,
        isRinging: Bool,
        accessibilityName: String,
        action: @escaping () -> Void
    ) -> some View {
        let button = Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: isRinging ? "speaker.slash.fill" : "speaker.wave.2.fill")
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 0) {
                    Text(isRinging ? "Mute" : "Ring")
                        .font(.callout.weight(.medium))
                    HStack(spacing: 3) {
                        Image(systemName: systemImage)
                        Text(side)
                    }
                    .font(.caption)
                    .foregroundStyle(isRinging ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                }
                Spacer(minLength: 0)
            }
            .padding(.vertical, 2)
            .frame(maxWidth: .infinity)
        }
        .accessibilityLabel(isRinging ? "Mute \(accessibilityName)" : "Ring \(accessibilityName)")

        if isRinging {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.glass)
        }
    }
}

#if DEBUG
#Preview {
    FindControl(
        find: FindSession(makeTransport: { SimulatedTransport.find() }),
        formFactor: .stemless,
        isExpanded: .constant(true)
    )
        .padding()
        .frame(width: 300)
}

#Preview("Headphones") {
    FindControl(
        find: FindSession(makeTransport: { SimulatedTransport.find() }),
        formFactor: .headphones,
        isExpanded: .constant(true)
    )
        .padding()
        .frame(width: 300)
}
#endif
