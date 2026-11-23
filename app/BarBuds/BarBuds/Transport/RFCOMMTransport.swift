import Foundation
import IOBluetooth
import Synchronization
import os

/// A Classic Bluetooth RFCOMM channel to one service on a paired device.
///
/// IOBluetooth delivers delegate callbacks on the run loop of the thread that
/// opened the channel, so every IOBluetooth call here runs on the main thread.
/// Callbacks only touch the `Mutex`-guarded state and the stream continuation.
nonisolated final class RFCOMMTransport: NSObject, EarbudsTransport, @unchecked Sendable {
    enum Failure: LocalizedError {
        case deviceNotFound
        case serviceNotFound
        case openFailed(IOReturn)
        case notOpen
        case writeFailed(IOReturn)

        var errorDescription: String? {
            switch self {
            case .deviceNotFound: "The earbuds are no longer paired."
            case .serviceNotFound: "The earbuds don’t offer a control channel."
            case .openFailed(let status): "Couldn’t open the control channel (\(Self.describe(status)))."
            case .notOpen: "The control channel is closed."
            case .writeFailed(let status): "Couldn’t send to the earbuds (\(Self.describe(status)))."
            }
        }

        private static func describe(_ status: IOReturn) -> String {
            "error " + String(UInt32(bitPattern: status), radix: 16)
        }
    }

    let incoming: AsyncStream<Data>
    private let continuation: AsyncStream<Data>.Continuation
    private let address: String
    private let serviceUUID: String

    private struct State {
        var channel: IOBluetoothRFCOMMChannel?
        var opening: CheckedContinuation<Void, any Error>?
    }
    private let state = Mutex(State())

    private static let log = Logger(subsystem: "com.nikhilprabhu.BarBuds", category: "rfcomm")

    /// - Parameters:
    ///   - address: The device's Bluetooth address, e.g. "b8-7b-d4-07-b6-9f".
    ///   - serviceUUID: The SDP service UUID of the control channel.
    init(address: String, serviceUUID: String) {
        (incoming, continuation) = AsyncStream.makeStream(of: Data.self)
        self.address = address
        self.serviceUUID = serviceUUID
        super.init()
    }

    static func sdpUUID(_ uuid: String) -> IOBluetoothSDPUUID? {
        guard var bytes = UUID(uuidString: uuid)?.uuid else { return nil }
        return IOBluetoothSDPUUID(bytes: &bytes, length: 16)
    }

    // MARK: EarbudsTransport

    func open() async throws {
        try await withCheckedThrowingContinuation { (opening: CheckedContinuation<Void, any Error>) in
            Task { @MainActor in self.beginOpen(opening) }
        }
    }

    func write(_ data: Data) async throws {
        try await MainActor.run { try self.writeOnMain(data) }
    }

    func close() async {
        await MainActor.run {
            let channel = state.withLock { state in
                defer { state.channel = nil }
                return state.channel
            }
            channel?.setDelegate(nil)
            channel?.close()
        }
        continuation.finish()
    }

    // MARK: Main thread

    @MainActor
    private func beginOpen(_ opening: CheckedContinuation<Void, any Error>) {
        guard let device = IOBluetoothDevice(addressString: address) else {
            return opening.resume(throwing: Failure.deviceNotFound)
        }
        var channelID: BluetoothRFCOMMChannelID = 0
        guard let uuid = Self.sdpUUID(serviceUUID),
              let record = device.getServiceRecord(for: uuid),
              record.getRFCOMMChannelID(&channelID) == kIOReturnSuccess
        else {
            return opening.resume(throwing: Failure.serviceNotFound)
        }

        state.withLock { $0.opening = opening }
        var channel: IOBluetoothRFCOMMChannel?
        let status = device.openRFCOMMChannelAsync(&channel, withChannelID: channelID, delegate: self)
        guard status == kIOReturnSuccess else {
            takeOpening()?.resume(throwing: Failure.openFailed(status))
            return
        }
        // The open-complete callback can't run before this returns: it's
        // delivered on this same (main) run loop.
        state.withLock { $0.channel = channel }
        Self.log.debug("Opening RFCOMM channel \(channelID) to \(self.address, privacy: .public)")
    }

    @MainActor
    private func writeOnMain(_ data: Data) throws {
        guard let channel = state.withLock({ $0.channel }) else { throw Failure.notOpen }
        Self.log.debug("TX \(Self.hex(data), privacy: .public)")
        let mtu = max(Int(channel.getMTU()), 1)
        var bytes = [UInt8](data)
        var offset = 0
        while offset < bytes.count {
            let length = min(mtu, bytes.count - offset)
            let status = bytes.withUnsafeMutableBytes { buffer in
                channel.writeSync(buffer.baseAddress! + offset, length: UInt16(length))
            }
            guard status == kIOReturnSuccess else { throw Failure.writeFailed(status) }
            offset += length
        }
    }

    private func takeOpening() -> CheckedContinuation<Void, any Error>? {
        state.withLock { state in
            defer { state.opening = nil }
            return state.opening
        }
    }

    private static func hex(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined(separator: " ")
    }
}

extension RFCOMMTransport: IOBluetoothRFCOMMChannelDelegate {
    func rfcommChannelOpenComplete(_ channel: IOBluetoothRFCOMMChannel!, status error: IOReturn) {
        if error == kIOReturnSuccess {
            takeOpening()?.resume()
        } else {
            state.withLock { $0.channel = nil }
            takeOpening()?.resume(throwing: Failure.openFailed(error))
        }
    }

    func rfcommChannelData(_ channel: IOBluetoothRFCOMMChannel!, data dataPointer: UnsafeMutableRawPointer!, length dataLength: Int) {
        let data = Data(bytes: dataPointer, count: dataLength)
        Self.log.debug("RX \(Self.hex(data), privacy: .public)")
        continuation.yield(data)
    }

    func rfcommChannelClosed(_ channel: IOBluetoothRFCOMMChannel!) {
        Self.log.debug("RFCOMM channel closed by the device")
        state.withLock { $0.channel = nil }
        takeOpening()?.resume(throwing: Failure.notOpen)
        continuation.finish()
    }
}
