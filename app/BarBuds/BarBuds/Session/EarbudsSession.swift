import BarBudsCore
import Foundation
import Observation
import os

/// UI-facing model for one pair of earbuds. Owns the Rust controller and a
/// transport, pumps bytes between them, and publishes the confirmed device state.
@Observable
final class EarbudsSession: Identifiable {
    enum Connection: Equatable {
        case disconnected
        case connecting
        case connected
    }

    let id: String
    let name: String
    let capabilities: DeviceCapabilities
    private(set) var connection: Connection = .disconnected
    /// Last state the device confirmed.
    private(set) var state: DeviceState
    private(set) var lastError: String?
    /// A custom EQ curve being edited that the device hasn't confirmed yet, so
    /// sliders track the pointer instead of waiting on a round trip.
    private var eqDraft: EqSettings?

    @ObservationIgnored private let controller: BarBudsController
    @ObservationIgnored private let makeTransport: @Sendable () -> any EarbudsTransport
    @ObservationIgnored private var transport: (any EarbudsTransport)?
    @ObservationIgnored private var readTask: Task<Void, Never>?
    @ObservationIgnored private var eqWriteTask: Task<Void, Never>?

    private static let eqDebounce: Duration = .milliseconds(200)
    private static let log = Logger(subsystem: "com.nikhilprabhu.BarBuds", category: "session")

    init(device: DiscoveredDevice) {
        id = device.id
        name = device.name
        controller = BarBudsController(model: device.model)
        capabilities = controller.capabilities()
        state = controller.state()
        makeTransport = device.makeTransport
    }

    /// The EQ to display: the in-progress edit if there is one, else the confirmed state.
    var eq: EqSettings? { eqDraft ?? state.eq }

    // MARK: Connection

    func connect() async {
        guard connection == .disconnected else { return }
        connection = .connecting
        lastError = nil

        let transport = makeTransport()
        do {
            try await transport.open()
        } catch {
            connection = .disconnected
            lastError = error.localizedDescription
            return
        }
        self.transport = transport
        connection = .connected

        readTask = Task { [weak self] in
            for await chunk in transport.incoming {
                guard let self else { return }
                // Some protocols answer replies with follow-up requests while connecting.
                let outbound = receive(chunk)
                if !outbound.isEmpty {
                    await write(outbound)
                }
            }
            self?.transportClosed(transport)
        }
        await write(controller.start())
    }

    func disconnect() async {
        guard let transport else { return }
        tearDown()
        await transport.close()
    }

    private func tearDown() {
        readTask?.cancel()
        readTask = nil
        eqWriteTask?.cancel()
        eqWriteTask = nil
        transport = nil
        eqDraft = nil
        controller.reset()
        state = controller.state()
        connection = .disconnected
    }

    private func transportClosed(_ closed: any EarbudsTransport) {
        // Ignore streams we finished ourselves in `disconnect()`.
        guard transport === closed else { return }
        tearDown()
        lastError = "Connection lost."
    }

    // MARK: Settings

    func setAnc(_ mode: AncMode) {
        Task { await send { try controller.encodeAnc(mode: mode) } }
    }

    func setToggle(_ toggle: FeatureToggle, on: Bool) {
        Task { await send { try controller.encodeToggle(toggle: toggle, on: on) } }
    }

    func selectEqPreset(_ preset: EqPreset) {
        eqWriteTask?.cancel()
        eqWriteTask = nil
        eqDraft = nil
        // Switching to Custom starts from the curve currently shown.
        let settings = EqSettings(preset: preset, gains: eq?.gains ?? [])
        Task { await send { try controller.encodeEq(eq: settings) } }
    }

    /// Moves one band, switching to the Custom preset. Writes are debounced so
    /// dragging a slider doesn't flood the Bluetooth channel.
    func setGain(_ gain: Float, forBandAt index: Int) {
        guard var draft = eq, draft.gains.indices.contains(index) else { return }
        draft.preset = .custom
        draft.gains[index] = gain
        eqDraft = draft

        eqWriteTask?.cancel()
        eqWriteTask = Task { [weak self] in
            try? await Task.sleep(for: Self.eqDebounce)
            guard !Task.isCancelled, let self else { return }
            eqWriteTask = nil
            await send { try self.controller.encodeEq(eq: draft) }
        }
    }

    // MARK: I/O

    /// Applies bytes read from the device. Returns any frames the protocol
    /// wants written in response.
    private func receive(_ chunk: Data) -> Data {
        let previousEq = state.eq
        do {
            state = try controller.feed(bytes: chunk)
        } catch {
            // Valid frames in the chunk were still applied.
            Self.log.error("Discarded bad frame: \(error.localizedDescription, privacy: .public)")
            state = controller.state()
        }
        // The device confirmed an EQ change and no newer edit is waiting to be sent.
        if eqWriteTask == nil, state.eq != previousEq {
            eqDraft = nil
        }
        return controller.takeOutbound()
    }

    private func send(_ encode: () throws -> Data) async {
        do {
            await write(try encode())
        } catch {
            lastError = error.localizedDescription
        }
    }

    private func write(_ frame: Data) async {
        guard let transport else { return }
        do {
            try await transport.write(frame)
        } catch {
            lastError = error.localizedDescription
        }
    }
}

#if DEBUG
extension EarbudsSession {
    /// A session already filled in from the simulated earbuds, for static
    /// previews. It has no transport, so changing settings does nothing.
    static func preview(name: String = "Mock Buds", id: String = "preview") -> EarbudsSession {
        let session = EarbudsSession(device: DiscoveredDevice(
            id: id, name: name, model: .mock, makeTransport: { MockTransport() }
        ))
        let reply = MockEarbuds().receive(bytes: session.controller.start())
        session.state = (try? session.controller.feed(bytes: reply)) ?? session.controller.state()
        session.connection = .connected
        return session
    }
}
#endif
