import Foundation

/// A byte pipe to one pair of earbuds: an RFCOMM/L2CAP channel, a GATT
/// characteristic, or the in-process mock. Transports only move bytes; all
/// framing and parsing happens in `BarBudsController`.
///
/// `nonisolated` opts out of the target's default `MainActor` isolation so that
/// actors and Bluetooth delegate types can conform.
nonisolated protocol EarbudsTransport: AnyObject, Sendable {
    /// Chunks read from the device, in order. Finishes when the channel closes.
    var incoming: AsyncStream<Data> { get }

    func open() async throws
    func write(_ data: Data) async throws
    func close() async
}
