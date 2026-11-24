import BarBudsCore
import Foundation
import Observation

/// Rings one pair of earbuds so the user can find them. Owns the Rust
/// `FindController` and a separate channel (the Fast Pair message stream),
/// which opens on the first ring and closes by itself once nothing is
/// ringing, so ringing can continue while the popover is closed.
@Observable
final class FindSession {
    enum Connection: Equatable {
        case closed
        case opening
        case open
    }

    private(set) var connection: Connection = .closed
    /// Which buds the earbuds confirmed are ringing.
    private(set) var ringing = RingState(left: false, right: false)
    private(set) var lastError: String?

    @ObservationIgnored private let controller = FindController()
    @ObservationIgnored private let makeTransport: @Sendable () -> any EarbudsTransport
    @ObservationIgnored private var transport: (any EarbudsTransport)?
    @ObservationIgnored private var readTask: Task<Void, Never>?
    /// A ring was sent and the earbuds haven't answered yet, so the channel
    /// must stay open even though nothing is ringing.
    @ObservationIgnored private var awaitingRing = false

    init(makeTransport: @escaping @Sendable () -> any EarbudsTransport) {
        self.makeTransport = makeTransport
    }

    var isRinging: Bool { ringing.left || ringing.right }

    /// Make exactly these buds ring (neither stops ringing).
    func ring(left: Bool, right: Bool) async {
        lastError = nil
        guard await openIfNeeded() else { return }
        awaitingRing = left || right
        await write(controller.ring(left: left, right: right))
    }

    func stop() async {
        guard transport != nil else { return }
        await write(controller.stop())
    }

    /// Closes the channel if nothing is ringing (e.g. a ring that never
    /// started). Ringing buds keep the channel until they stop.
    func closeIfIdle() async {
        if !isRinging, !awaitingRing {
            await close()
        }
    }

    /// Stops any ringing and closes the channel.
    func close() async {
        guard let transport else { return }
        if isRinging {
            await write(controller.stop())
        }
        readTask?.cancel()
        readTask = nil
        self.transport = nil
        connection = .closed
        ringing = RingState(left: false, right: false)
        await transport.close()
    }

    private func openIfNeeded() async -> Bool {
        if transport != nil { return true }
        guard connection == .closed else { return false }
        connection = .opening
        let transport = makeTransport()
        do {
            try await transport.open()
        } catch {
            connection = .closed
            lastError = error.localizedDescription
            return false
        }
        self.transport = transport
        connection = .open
        readTask = Task { [weak self] in
            for await chunk in transport.incoming {
                guard let self else { return }
                receive(chunk)
                let outbound = controller.takeOutbound()
                if !outbound.isEmpty {
                    await write(outbound)
                }
            }
            self?.channelClosed(transport)
        }
        return true
    }

    private func receive(_ chunk: Data) {
        let wasRinging = isRinging
        var refused = false
        do {
            ringing = try controller.feed(bytes: chunk)
        } catch {
            ringing = controller.state()
            lastError = error.localizedDescription
            awaitingRing = false
            refused = true
        }
        if isRinging {
            awaitingRing = false
        }
        // Both buds stopped (muted, or timed out on their own), or the ring was
        // refused: release the channel. It reopens on the next ring.
        if wasRinging || refused, !isRinging {
            Task { await closeIfIdle() }
        }
    }

    private func channelClosed(_ closed: any EarbudsTransport) {
        guard transport === closed else { return }
        transport = nil
        readTask = nil
        connection = .closed
        ringing = RingState(left: false, right: false)
    }

    private func write(_ data: Data) async {
        guard let transport else { return }
        do {
            try await transport.write(data)
        } catch {
            lastError = error.localizedDescription
        }
    }
}
