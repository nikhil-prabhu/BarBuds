import BarBudsCore
import Foundation

/// The device side of a simulated channel, implemented in Rust:
/// `DeviceSimulator` (a model's control channel) or `FindSimulator` (the
/// Fast Pair ring feature).
nonisolated protocol SimulatedDevice: AnyObject, Sendable {
    func receive(bytes: Data) -> Data
    func tick() -> Data
}

extension DeviceSimulator: SimulatedDevice {}
extension FindSimulator: SimulatedDevice {}

/// Stands in for a Bluetooth channel by routing bytes through a Rust
/// simulator, which speaks the real protocol, with a little latency so the UI
/// behaves as it would with real hardware (settings change only once the
/// device confirms).
actor SimulatedTransport: EarbudsTransport {
    nonisolated let incoming: AsyncStream<Data>
    private let continuation: AsyncStream<Data>.Continuation
    private let device: any SimulatedDevice
    private let latency: Duration
    private let tickInterval: Duration
    private var ticker: Task<Void, Never>?

    /// A simulated control channel for `model`, whose buds drain 1% battery
    /// every `tickInterval`.
    init(model: DeviceModel, tickInterval: Duration = .seconds(20)) {
        self.init(device: DeviceSimulator(model: model), tickInterval: tickInterval)
    }

    /// - Parameters:
    ///   - latency: Delay before the simulated device replies.
    ///   - tickInterval: How often simulated time passes (battery drain,
    ///     a ring stopping by itself).
    init(
        device: any SimulatedDevice,
        latency: Duration = .milliseconds(40),
        tickInterval: Duration
    ) {
        (incoming, continuation) = AsyncStream.makeStream(of: Data.self)
        self.device = device
        self.latency = latency
        self.tickInterval = tickInterval
    }

    func open() async throws {
        try await Task.sleep(for: latency)
        ticker = Task { [device, continuation, tickInterval] in
            while true {
                do {
                    try await Task.sleep(for: tickInterval)
                } catch {
                    return
                }
                continuation.yield(device.tick())
            }
        }
    }

    func write(_ data: Data) async throws {
        let reply = device.receive(bytes: data)
        guard !reply.isEmpty else { return }
        try await Task.sleep(for: latency)
        continuation.yield(reply)
    }

    func close() async {
        ticker?.cancel()
        ticker = nil
        continuation.finish()
    }
}

extension SimulatedTransport {
    /// A simulated Fast Pair channel for ringing the buds. A ring stops by
    /// itself after about 15 seconds, like real earbuds.
    static func find() -> SimulatedTransport {
        SimulatedTransport(device: FindSimulator(), tickInterval: .seconds(1))
    }
}
