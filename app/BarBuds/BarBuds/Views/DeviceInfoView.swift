import BarBudsCore
import SwiftUI

/// Model, Bluetooth address, firmware and serial numbers of the earbuds
/// currently selected in the popover.
struct DeviceInfoView: View {
    let devices: DeviceManager

    var body: some View {
        Group {
            if let session = devices.selected {
                DeviceInfoForm(session: session)
                    .id(session.id)
            } else {
                ContentUnavailableView("No Earbuds Connected", systemImage: "earbuds")
                    .frame(height: 200)
            }
        }
        .frame(width: 380)
        // One exact height, so the window (sized to its content) always fits it.
        .fixedSize(horizontal: false, vertical: true)
    }
}

private struct DeviceInfoForm: View {
    let session: EarbudsSession
    /// Serial numbers end up in shared screenshots, so they stay hidden until asked for.
    @State private var showSerialNumbers = false

    private var info: DeviceInfo { session.state.info }

    var body: some View {
        // Plain stacks rather than a `Form`: a grouped Form scrolls, so it has no
        // fixed height and the window could open too short for its rows.
        VStack(alignment: .leading, spacing: 20) {
            InfoSection {
                InfoRow("Name", session.name)
                InfoRow("Model", session.capabilities.displayName)
                if let address = bluetoothAddress {
                    InfoRow("Bluetooth Address", address)
                }
            }

            InfoSection("Firmware") {
                ComponentRows(
                    values: info.firmware,
                    combinedLabel: "Version",
                    hasCase: session.capabilities.hasChargingCase
                )
            }

            InfoSection("Serial Numbers") {
                // Always laid out, masked until shown, so revealing them
                // doesn't change the window's size.
                ComponentRows(
                    values: info.serialNumbers,
                    combinedLabel: "Serial Number",
                    hasCase: session.capabilities.hasChargingCase,
                    combineIdentical: false,
                    masked: !showSerialNumbers
                )
            } accessory: {
                Button(showSerialNumbers ? "Hide" : "Show") {
                    showSerialNumbers.toggle()
                }
                .buttonStyle(.link)
                .font(.callout)
            }
        }
        .padding(20)
    }

    /// "B8:7B:D4:…" for Bluetooth devices; `nil` for simulated ones.
    private var bluetoothAddress: String? {
        let parts = session.id.split(separator: "-")
        guard parts.count == 6, parts.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isHexDigit) }) else {
            return nil
        }
        return parts.joined(separator: ":").uppercased()
    }
}

/// One row per component that reported a value, or a single row when every
/// component (including the case) reports the same one, as firmware usually does.
private struct ComponentRows: View {
    let values: ComponentStrings
    let combinedLabel: String
    let hasCase: Bool
    var combineIdentical = true
    /// Shows dots in place of each value.
    var masked = false

    private var components: [(label: String, value: String?)] {
        var rows = [("Left", values.left), ("Right", values.right)]
        if hasCase {
            rows.append(("Case", values.chargingCase))
        }
        return rows
    }

    var body: some View {
        let reported = components.filter { $0.value != nil }
        let distinct = Set(reported.compactMap(\.value))
        if reported.isEmpty {
            Text("Not reported yet")
                .foregroundStyle(.secondary)
        } else if combineIdentical, reported.count == components.count, distinct.count == 1 {
            row(combinedLabel, reported[0].value!)
        } else {
            ForEach(reported, id: \.label) { component in
                row(component.label, component.value!)
            }
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        InfoRow(label, masked ? String(repeating: "•", count: 10) : value)
    }
}

/// A titled, rounded group of rows separated by dividers, like a grouped form section.
private struct InfoSection<Content: View, Accessory: View>: View {
    let title: String?
    @ViewBuilder let content: Content
    @ViewBuilder let accessory: Accessory

    init(
        _ title: String? = nil,
        @ViewBuilder content: () -> Content,
        @ViewBuilder accessory: () -> Accessory = { EmptyView() }
    ) {
        self.title = title
        self.content = content()
        self.accessory = accessory()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let title {
                HStack {
                    Text(title)
                        .font(.headline)
                    Spacer()
                    accessory
                }
                .padding(.horizontal, 4)
            }
            VStack(spacing: 0) {
                Group(subviews: content) { rows in
                    ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                        if index > 0 {
                            Divider()
                        }
                        row
                            .padding(.vertical, 8)
                    }
                }
            }
            .padding(.horizontal, 12)
            .background(.fill.quaternary, in: .rect(cornerRadius: 10))
        }
    }
}

private struct InfoRow: View {
    let label: String
    let value: String

    init(_ label: String, _ value: String) {
        self.label = label
        self.value = value
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
            Spacer(minLength: 16)
            Text(value)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .textSelection(.enabled)
                .multilineTextAlignment(.trailing)
        }
    }
}

#Preview {
    let firmware = "release_5.11_signed"
    DeviceInfoView(devices: DeviceManager(previewSessions: [
        .preview(
            name: "Nikhil's Buds",
            id: "aa-bb-cc-dd-ee-ff",
            info: DeviceInfo(
                firmware: ComponentStrings(left: firmware, right: firmware, chargingCase: firmware),
                serialNumbers: ComponentStrings(left: "SIMLEFT01", right: "SIMRIGHT01", chargingCase: "SIMCASE01")
            )
        ),
    ]))
}
