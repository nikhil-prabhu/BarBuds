import BarBudsCore
import Foundation
import IOBluetooth
import os

/// Reports paired Classic Bluetooth earbuds that BarBuds has a driver for, as
/// they connect to and disconnect from the Mac.
///
/// A device is recognized by the control-channel service its SDP records
/// advertise (`rfcommServiceUuid(model:)` from the core), so a renamed pair
/// is still found.
nonisolated struct BluetoothDiscovery: DeviceDiscovery {
    func events() -> AsyncStream<DiscoveryEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: DiscoveryEvent.self)
        Task { @MainActor in
            let monitor = BluetoothMonitor(continuation: continuation)
            // The termination handler keeps the monitor alive as long as the stream.
            continuation.onTermination = { _ in
                Task { @MainActor in monitor.stop() }
            }
            monitor.start()
        }
        return stream
    }
}

/// Owns the IOBluetooth notification registrations. IOBluetooth calls the
/// selectors on the main run loop.
private final class BluetoothMonitor: NSObject {
    private let continuation: AsyncStream<DiscoveryEvent>.Continuation
    private var connectNotification: IOBluetoothUserNotification?
    /// One disconnect registration per reported device, keyed by address.
    private var reported: [String: IOBluetoothUserNotification] = [:]

    private static let log = Logger(subsystem: "com.nikhilprabhu.BarBuds", category: "discovery")

    init(continuation: AsyncStream<DiscoveryEvent>.Continuation) {
        self.continuation = continuation
    }

    func start() {
        connectNotification = IOBluetoothDevice.register(
            forConnectNotifications: self,
            selector: #selector(deviceConnected(_:device:))
        )
        let paired = IOBluetoothDevice.pairedDevices() as? [IOBluetoothDevice] ?? []
        for device in paired where device.isConnected() {
            report(device)
        }
    }

    func stop() {
        connectNotification?.unregister()
        connectNotification = nil
        reported.values.forEach { $0.unregister() }
        reported.removeAll()
    }

    @objc private func deviceConnected(_ notification: IOBluetoothUserNotification, device: IOBluetoothDevice) {
        report(device)
    }

    @objc private func deviceDisconnected(_ notification: IOBluetoothUserNotification, device: IOBluetoothDevice) {
        guard let id = device.addressString else { return }
        reported.removeValue(forKey: id)?.unregister()
        continuation.yield(.disconnected(id: id))
    }

    private func report(_ device: IOBluetoothDevice) {
        guard let id = device.addressString, reported[id] == nil else { return }
        guard let (model, serviceUUID) = Self.supportedModel(of: device) else {
            Self.log.debug("Ignoring \(device.name ?? id, privacy: .public): no supported control service")
            return
        }
        reported[id] = device.register(
            forDisconnectNotification: self,
            selector: #selector(deviceDisconnected(_:device:))
        )
        let name = device.name ?? BarBudsController(model: model).capabilities().displayName
        Self.log.info("Found \(name, privacy: .public) (\(String(describing: model), privacy: .public))")
        continuation.yield(.connected(DiscoveredDevice(
            id: id,
            name: name,
            model: model,
            makeTransport: { RFCOMMTransport(address: id, serviceUUID: serviceUUID) }
        )))
    }

    /// The first supported model whose control service the device advertises.
    /// Uses macOS's cached SDP records, which exist for any paired device.
    private static func supportedModel(of device: IOBluetoothDevice) -> (DeviceModel, String)? {
        for model in supportedModels() {
            guard let uuid = rfcommServiceUuid(model: model),
                  let sdpUUID = RFCOMMTransport.sdpUUID(uuid),
                  device.getServiceRecord(for: sdpUUID) != nil
            else { continue }
            return (model, uuid)
        }
        return nil
    }
}
