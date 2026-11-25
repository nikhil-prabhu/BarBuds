import Foundation

/// Lets `FindSession` ring over a session's own control channel when that
/// channel is the Fast Pair message stream (the generic Fast Pair driver),
/// since one RFCOMM channel can't be opened twice. The session forwards every
/// chunk it reads here and sets `write` while connected; each
/// `SharedChannelTransport` sees the chunks and writes through it.
final class SharedChannel {
    /// Writes to the control channel; `nil` while it's closed.
    var write: (@Sendable (Data) async throws -> Void)?
    private var listeners: [UUID: AsyncStream<Data>.Continuation] = [:]

    /// A chunk read from the control channel.
    func deliver(_ chunk: Data) {
        for listener in listeners.values {
            listener.yield(chunk)
        }
    }

    /// The control channel closed: ends every attached transport.
    func closed() {
        write = nil
        for listener in listeners.values {
            listener.finish()
        }
        listeners.removeAll()
    }

    fileprivate func attach(_ id: UUID, _ listener: AsyncStream<Data>.Continuation) throws {
        guard write != nil else { throw SharedChannelError.notConnected }
        listeners[id] = listener
    }

    fileprivate func detach(_ id: UUID) {
        listeners.removeValue(forKey: id)?.finish()
    }

    fileprivate func send(_ data: Data) async throws {
        guard let write else { throw SharedChannelError.notConnected }
        try await write(data)
    }
}

enum SharedChannelError: LocalizedError {
    case notConnected

    var errorDescription: String? {
        "The earbuds aren't connected."
    }
}

/// A transport over a `SharedChannel`. Closing it detaches from the channel
/// without closing the channel itself.
nonisolated final class SharedChannelTransport: EarbudsTransport {
    let incoming: AsyncStream<Data>
    private let continuation: AsyncStream<Data>.Continuation
    private let channel: SharedChannel
    private let id = UUID()

    init(channel: SharedChannel) {
        self.channel = channel
        (incoming, continuation) = AsyncStream.makeStream(of: Data.self)
    }

    func open() async throws {
        try await channel.attach(id, continuation)
    }

    func write(_ data: Data) async throws {
        try await channel.send(data)
    }

    func close() async {
        await channel.detach(id)
        continuation.finish()
    }
}
