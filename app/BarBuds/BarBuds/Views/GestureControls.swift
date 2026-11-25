import BarBudsCore
import SwiftUI

/// Remappable gestures, e.g. what a double pinch on the left bud does,
/// grouped by bud in a section that can be collapsed, plus which modes a
/// noise-control gesture cycles through.
struct GestureControls: View {
    let capabilities: ControlCapabilities
    /// Confirmed actions. A gesture the device hasn't reported is disabled.
    let settings: [ControlSetting]
    let formFactor: FormFactor
    @Binding var isExpanded: Bool
    let onSet: (ControlSetting) -> Void
    /// Modes the noise-control cycle can include; empty when it's fixed.
    var ancCycleModes: [AncMode] = []
    /// The confirmed cycle; `nil` until the device reports it.
    var ancCycle: [AncMode]? = nil
    var onSetAncCycle: ([AncMode]) -> Void = { _ in }

    /// The core's `ANC_CYCLE_MIN_MODES`: the vendor apps need at least two.
    private static let minimumCycleModes = 2

    var body: some View {
        CollapsibleSection(title: "Controls", isExpanded: $isExpanded) {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(sides, id: \.self) { side in
                    group(side)
                }
                // Only relevant while a gesture cycles noise control.
                if !ancCycleModes.isEmpty, settings.contains(where: { $0.action == .noiseControl }) {
                    cycleGroup
                }
            }
        }
    }

    /// The sides that have gestures, left first.
    private var sides: [BudSide] {
        [.left, .right].filter { side in capabilities.slots.contains { $0.side == side } }
    }

    private func group(_ side: BudSide) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(side.title, systemImage: side == .left ? formFactor.leftSymbol : formFactor.rightSymbol)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ForEach(capabilities.slots.filter { $0.side == side }, id: \.gesture) { slot in
                row(slot)
            }
        }
    }

    private var cycleGroup: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Noise Control Cycle")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ForEach(ancCycleModes, id: \.self) { mode in
                let included = ancCycle?.contains(mode) ?? false
                Toggle(mode.title, isOn: Binding(
                    get: { included },
                    set: { on in
                        let cycle = ancCycle ?? []
                        onSetAncCycle(on ? cycle + [mode] : cycle.filter { $0 != mode })
                    }
                ))
                .toggleStyle(.checkbox)
                // The last modes left can't be removed.
                .disabled(ancCycle == nil || (included && (ancCycle?.count ?? 0) <= Self.minimumCycleModes))
            }
            Text("Modes a noise-control gesture switches between. Choose at least two.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func row(_ slot: ControlSlot) -> some View {
        let current = settings.first { $0.side == slot.side && $0.gesture == slot.gesture }?.action
        return HStack {
            Text(slot.gesture.title(for: capabilities.input))
                .lineLimit(1)
            Spacer(minLength: 8)
            Picker(slot.gesture.title(for: capabilities.input), selection: Binding(
                get: { current },
                set: { action in
                    guard let action else { return }
                    onSet(ControlSetting(side: slot.side, gesture: slot.gesture, action: action))
                }
            )) {
                ForEach(slot.actions, id: \.self) { action in
                    Text(action.title).tag(Optional(action))
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .fixedSize()
            .disabled(current == nil)
            .accessibilityLabel("\(slot.side.title) \(slot.gesture.title(for: capabilities.input))")
        }
    }
}

#Preview {
    let actions: [ControlAction] = [.skipBack, .skipForward, .voiceAssistant]
    GestureControls(
        capabilities: ControlCapabilities(
            input: .pinch,
            slots: [BudSide.left, .right].flatMap { side in
                [
                    ControlSlot(side: side, gesture: .double, actions: actions),
                    ControlSlot(side: side, gesture: .hold, actions: [.noiseControl, .voiceAssistant]),
                ]
            }
        ),
        settings: [
            ControlSetting(side: .left, gesture: .double, action: .skipForward),
            ControlSetting(side: .left, gesture: .hold, action: .voiceAssistant),
            ControlSetting(side: .right, gesture: .double, action: .voiceAssistant),
            ControlSetting(side: .right, gesture: .hold, action: .noiseControl),
        ],
        formFactor: .earHook,
        isExpanded: .constant(true),
        onSet: { _ in },
        ancCycleModes: [.off, .noiseCancelling, .transparency],
        ancCycle: [.noiseCancelling, .transparency]
    )
    .padding()
    .frame(width: 300)
}
