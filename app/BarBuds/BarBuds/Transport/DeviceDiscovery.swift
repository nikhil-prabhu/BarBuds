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
    /// Opens the Fast Pair channel used to ring the buds, for models whose
    /// capabilities say `canRing`. `nil` when the device can't be rung.
    var makeFindTransport: (@Sendable () -> any EarbudsTransport)? = nil
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

/// Finds nothing, for previews: they supply their own sessions, and the
/// Debug menu's simulated devices are added directly.
nonisolated struct NoDiscovery: DeviceDiscovery {
    func events() -> AsyncStream<DiscoveryEvent> {
        // Left open, like a real source that may still report changes.
        AsyncStream { _ in }
    }
}
