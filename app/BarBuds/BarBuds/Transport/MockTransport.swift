import BarBudsCore
import Foundation

/// Stands in for a Bluetooth channel by routing bytes through the Rust
/// `MockEarbuds` simulator, with a little latency so the UI behaves as it
/// would with real hardware (settings change only once the device confirms).
actor MockTransport: EarbudsTransport {
    nonisolated let incoming: AsyncStream<Data>
    private let continuation: AsyncStream<Data>.Continuation
    private let device: MockEarbuds
    private let latency: Duration
    private let tickInterval: Duration
    private var ticker: Task<Void, Never>?

    /// - Parameters:
    ///   - latency: Delay before the simulated device replies.
    ///   - tickInterval: How often the simulated buds drain 1% battery and report it.
    init(
        device: MockEarbuds = MockEarbuds(),
        latency: Duration = .milliseconds(40),
        tickInterval: Duration = .seconds(20)
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
