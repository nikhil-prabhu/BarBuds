import BarBudsCore
import Foundation

/// A pair of earbuds that is connected to the Mac and has a supported driver.
nonisolated struct DiscoveredDevice: Identifiable, Sendable {
    /// Stable per device, e.g. the Bluetooth address.
    let id: String
    /// The user-facing name, e.g. "Nikhil's Buds".
    let name: String
    let model: DeviceModel
    /// Opens a fresh control channel to this device.
    let makeTransport: @Sendable () -> any EarbudsTransport
}

nonisolated enum DiscoveryEvent: Sendable {
    case connected(DiscoveredDevice)
    case disconnected(id: String)
}

/// Reports earbuds as they connect to and disconnect from the Mac. A source
/// first reports every device that is already connected, then reports changes.
/// See `BluetoothDiscovery` for real hardware.
nonisolated protocol DeviceDiscovery: Sendable {
    func events() -> AsyncStream<DiscoveryEvent>
}

/// Reports simulated earbuds as already connected, for UI development.
nonisolated struct MockDiscovery: DeviceDiscovery {
    var names: [String] = ["Mock Buds"]

    func events() -> AsyncStream<DiscoveryEvent> {
        AsyncStream { continuation in
            for (index, name) in names.enumerated() {
                continuation.yield(.connected(DiscoveredDevice(
                    id: "mock-\(index)",
                    name: name,
                    model: .mock,
                    makeTransport: { SimulatedTransport(model: .mock) }
                )))
            }
            // Leave the stream open: a real source keeps reporting changes.
        }
    }
}
