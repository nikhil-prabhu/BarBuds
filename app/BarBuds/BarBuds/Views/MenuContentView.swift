import BarBudsCore
import SwiftUI

/// The popover shown from the menu bar icon.
struct MenuContentView: View {
    let devices: DeviceManager

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            MenuHeader(devices: devices)

            if let session = devices.selected {
                DeviceControls(session: session)
                    .id(session.id)
            } else {
                noDevices
            }
        }
        .padding(16)
        .frame(width: 300)
        // The login item may have been changed in System Settings meanwhile.
        .onAppear { LaunchAtLogin.shared.refresh() }
        // Ringing buds keep their channel open after the popover closes;
        // anything else can let go of it.
        .onDisappear {
            for session in devices.sessions {
                Task { await session.find?.closeIfIdle() }
            }
        }
    }

    private var noDevices: some View {
        VStack(spacing: 8) {
            Image(systemName: "earbuds")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text("No Earbuds Connected")
                .font(.headline)
            Text("Connect supported earbuds in Bluetooth settings and they’ll appear here.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
    }
}

/// Battery, noise control and EQ for one device.
private struct DeviceControls: View {
    let session: EarbudsSession
    /// Shared by every device and remembered across launches.
    @AppStorage("settingsExpanded") private var settingsExpanded = false
    @AppStorage("findExpanded") private var findExpanded = false
    @AppStorage("balanceExpanded") private var balanceExpanded = false
    @AppStorage("controlsExpanded") private var controlsExpanded = false

    var body: some View {
        switch session.connection {
        case .connected:
            connected
        case .connecting:
            ProgressView("Connecting…")
                .controlSize(.small)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 24)
        case .disconnected:
            VStack(spacing: 8) {
                Text(session.lastError ?? "Not connected.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Button("Reconnect") {
                    Task { await session.connect() }
                }
                .buttonStyle(.glassProminent)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
        }
    }

    @ViewBuilder
    private var connected: some View {
        let capabilities = session.capabilities

        BatteryView(
            battery: session.state.battery,
            hasChargingCase: capabilities.hasChargingCase,
            formFactor: capabilities.formFactor
        )

        if !capabilities.ancModes.isEmpty {
            Divider()
            AncControl(
                modes: capabilities.ancModes,
                selected: session.state.anc,
                availability: session.state.ancAvailability,
                deviceNoun: capabilities.formFactor.deviceNoun,
                onSelect: session.setAnc
            )
        }

        if !capabilities.spatialAudioModes.isEmpty {
            Divider()
            SpatialAudioControl(
                modes: capabilities.spatialAudioModes,
                selected: session.state.spatialAudio,
                onSelect: session.setSpatialAudio
            )
        }

        if let eqCapabilities = capabilities.eq, let eq = session.eq {
            Divider()
            EqControl(
                capabilities: eqCapabilities,
                settings: eq,
                onSelectPreset: session.selectEqPreset,
                onSetGain: session.setGain
            )
        }

        if capabilities.hasBalance {
            Divider()
            BalanceSlider(
                value: session.balance,
                isExpanded: $balanceExpanded,
                onChange: session.setBalance
            )
        }

        if !capabilities.toggles.isEmpty {
            Divider()
            FeatureToggles(
                toggles: capabilities.toggles,
                values: session.state.toggles,
                isExpanded: $settingsExpanded,
                onSet: { session.setToggle($0, on: $1) },
                formFactor: capabilities.formFactor
            )
        }

        if let controls = capabilities.controls {
            Divider()
            GestureControls(
                capabilities: controls,
                settings: session.state.controls,
                formFactor: capabilities.formFactor,
                isExpanded: $controlsExpanded,
                onSet: session.setControl,
                ancCycleModes: capabilities.ancCycleModes,
                ancCycle: session.state.ancCycle,
                onSetAncCycle: session.setAncCycle
            )
        }

        if let find = session.find {
            Divider()
            FindControl(find: find, formFactor: capabilities.formFactor, isExpanded: $findExpanded)
        }

        if let error = session.lastError {
            Label(error, systemImage: "exclamationmark.triangle")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

private struct MenuHeader: View {
    let devices: DeviceManager
    @Environment(\.openWindow) private var openWindow
    private let launchAtLogin = LaunchAtLogin.shared
    #if DEBUG
    @AppStorage(BluetoothDiscovery.preferFastPairKey) private var preferFastPair = false
    #endif

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            if let session = devices.selected {
                Image(systemName: session.capabilities.formFactor.pairSymbol)
                    .font(.title2)
                    .foregroundStyle(.secondary)
                    .frame(width: 32)
                    .accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 2) {
                title
                if let session = devices.selected {
                    Text(status(of: session))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer()
            Menu {
                #if DEBUG
                Section("Debug") {
                    Menu("Simulate Device Connecting") {
                        ForEach(supportedModels(), id: \.self) { model in
                            if model == .nothing {
                                // One driver, many models: pick which to simulate.
                                Menu("Nothing and CMF") {
                                    ForEach(nothingModels(), id: \.id) { nothing in
                                        Button(nothing.tested ? nothing.name : "\(nothing.name) (Untested)") {
                                            devices.simulateConnect(nothing: nothing)
                                        }
                                    }
                                }
                            } else {
                                Button(BarBudsController(model: model).capabilities().displayName) {
                                    devices.simulateConnect(model)
                                }
                                if model == .pixelBudsPro {
                                    // Same protocol; recognised by its name.
                                    Button("Pixel Buds Pro 2 (Untested)") {
                                        devices.simulateConnect(model, named: "Pixel Buds Pro 2")
                                    }
                                }
                            }
                        }
                    }
                    Button("Simulate Device Disconnecting", action: devices.simulateDisconnect)
                        .disabled(devices.selected == nil)
                    Toggle("Prefer Fast Pair Driver (Applies on Relaunch)", isOn: $preferFastPair)
                }
                #endif
                Button("About These \(devices.selected?.capabilities.formFactor.deviceTitle ?? "Earbuds")") {
                    NSApp.activate()
                    openWindow(id: "device-info")
                }
                .disabled(devices.selected == nil)
                Button("About BarBuds") {
                    NSApp.activate()
                    openWindow(id: "about")
                }
                Divider()
                Toggle("Open at Login", isOn: Binding(
                    get: { launchAtLogin.isEnabled },
                    set: { launchAtLogin.setEnabled($0) }
                ))
                Divider()
                Button("Quit BarBuds") {
                    NSApplication.shared.terminate(nil)
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .foregroundStyle(.secondary)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
    }

    /// The device name, or a switcher when more than one device is connected.
    @ViewBuilder
    private var title: some View {
        let name = devices.selected?.name ?? "BarBuds"
        if devices.sessions.count > 1 {
            Menu {
                ForEach(devices.sessions) { session in
                    Toggle(isOn: Binding(
                        get: { session.id == devices.selected?.id },
                        set: { if $0 { devices.selectedID = session.id } }
                    )) {
                        Label(session.name, systemImage: session.capabilities.formFactor.pairSymbol)
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    // Long names truncate instead of widening the popover.
                    Text(name)
                        .font(.headline)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .fixedSize()
                }
            }
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
        } else {
            Text(name)
                .font(.headline)
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }

    private func status(of session: EarbudsSession) -> String {
        let model = session.capabilities.displayName
        let detail = switch session.connection {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .disconnected: "Disconnected"
        }
        // Only repeat the model when the user has renamed the device.
        return session.name == model ? detail : "\(model) · \(detail)"
    }
}

// The previews use DEBUG-only helpers (`EarbudsSession.preview`,
// `DeviceManager(previewSessions:)`), so Release builds skip them.
#if DEBUG
#Preview("Pixel Buds Pro") {
    MenuContentView(devices: DeviceManager(previewSessions: [.preview(model: .pixelBudsPro)]))
}

#Preview("Pixel Buds A-Series") {
    MenuContentView(devices: DeviceManager(previewSessions: [.preview(model: .pixelBudsASeries)]))
}

#Preview("Nothing Ear (open)") {
    MenuContentView(devices: DeviceManager(previewSessions: [.preview(model: .nothing, name: "Nothing Ear (open)")]))
}

#Preview("Fast Pair device") {
    MenuContentView(devices: DeviceManager(previewSessions: [.preview(model: .fastPair)]))
}

#Preview("Two devices") {
    MenuContentView(devices: DeviceManager(previewSessions: [
        .preview(model: .pixelBudsPro, name: "Work Buds", id: "a"),
        .preview(model: .pixelBudsASeries, name: "Gym Buds", id: "b"),
    ]))
}

#Preview("Long names") {
    MenuContentView(devices: DeviceManager(previewSessions: [
        .preview(model: .pixelBudsASeries, name: "Simulated Pixel Buds A-Series", id: "a"),
        .preview(model: .pixelBudsPro, name: "Someone's Extremely Long Earbud Name", id: "b"),
    ]))
}

#Preview("No devices") {
    MenuContentView(devices: DeviceManager(previewSessions: []))
}
#endif
