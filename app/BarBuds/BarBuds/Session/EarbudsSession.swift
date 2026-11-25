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
    /// Rings the buds to find them; `nil` for models that can't be rung.
    let find: FindSession?
    /// A custom EQ curve being edited that the device hasn't confirmed yet, so
    /// sliders track the pointer instead of waiting on a round trip.
    private var eqDraft: EqSettings?
    /// A balance being dragged that the device hasn't confirmed yet.
    private var balanceDraft: Int32?

    @ObservationIgnored private let controller: BarBudsController
    @ObservationIgnored private let makeTransport: @Sendable () -> any EarbudsTransport
    @ObservationIgnored private var transport: (any EarbudsTransport)?
    @ObservationIgnored private var readTask: Task<Void, Never>?
    @ObservationIgnored private var eqWriteTask: Task<Void, Never>?
    @ObservationIgnored private var balanceWriteTask: Task<Void, Never>?

    private static let eqDebounce: Duration = .milliseconds(200)
    private static let log = Logger(subsystem: "com.nikhilprabhu.BarBuds", category: "session")

    init(device: DiscoveredDevice) {
        id = device.id
        name = device.name
        controller = BarBudsController(model: device.model)
        capabilities = controller.capabilities()
        state = controller.state()
        makeTransport = device.makeTransport
        if capabilities.canRing, let makeFindTransport = device.makeFindTransport {
            find = FindSession(makeTransport: makeFindTransport)
        } else {
            find = nil
        }
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
        await find?.close()
        guard let transport else { return }
        tearDown()
        await transport.close()
    }

    private func tearDown() {
        readTask?.cancel()
        readTask = nil
        eqWriteTask?.cancel()
        eqWriteTask = nil
        balanceWriteTask?.cancel()
        balanceWriteTask = nil
        transport = nil
        eqDraft = nil
        balanceDraft = nil
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

    /// Changes what a gesture does; shown once the device confirms it.
    func setControl(_ setting: ControlSetting) {
        Task { await send { try controller.encodeControl(setting: setting) } }
    }

    /// Selects a preset by ID, or a custom curve for `nil`.
    func selectEqPreset(_ presetID: String?) {
        eqWriteTask?.cancel()
        eqWriteTask = nil
        eqDraft = nil
        // Switching to a custom curve starts from the curve currently shown.
        let settings = EqSettings(preset: presetID, gains: eq?.gains ?? [])
        Task { await send { try controller.encodeEq(eq: settings) } }
    }

    /// Moves one band, switching to a custom curve. Writes are debounced so
    /// dragging a slider doesn't flood the Bluetooth channel.
    func setGain(_ gain: Float, forBandAt index: Int) {
        guard var draft = eq, draft.gains.indices.contains(index) else { return }
        draft.preset = nil
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

    /// The balance to display: the in-progress drag if there is one, else
    /// the confirmed value. -100 = only left … 0 = centred … 100 = only right.
    var balance: Int32? { balanceDraft ?? state.balance }

    /// Moves the balance. Writes are debounced like EQ sliders.
    func setBalance(_ value: Int32) {
        balanceDraft = value
        balanceWriteTask?.cancel()
        balanceWriteTask = Task { [weak self] in
            try? await Task.sleep(for: Self.eqDebounce)
            guard !Task.isCancelled, let self else { return }
            balanceWriteTask = nil
            await send { try self.controller.encodeBalance(balance: value) }
        }
    }

    // MARK: I/O

    /// Applies bytes read from the device. Returns any frames the protocol
    /// wants written in response.
    private func receive(_ chunk: Data) -> Data {
        let previousEq = state.eq
        let previousBalance = state.balance
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
        // Same for balance; also when the device confirms the value dragged to.
        if balanceWriteTask == nil,
           state.balance != previousBalance || state.balance == balanceDraft {
            balanceDraft = nil
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
    /// A session already filled in from a simulated `model`, for static
    /// previews. It has no transport, so changing settings does nothing.
    static func preview(
        model: DeviceModel = .mock,
        name: String? = nil,
        id: String = "preview",
        info: DeviceInfo? = nil
    ) -> EarbudsSession {
        let displayName = BarBudsController(model: model).capabilities().displayName
        let session = EarbudsSession(device: DiscoveredDevice(
            id: id,
            name: name ?? displayName,
            model: model,
            makeTransport: { SimulatedTransport(model: model) },
            makeFindTransport: { SimulatedTransport.find() }
        ))
        // Run the handshake synchronously: some protocols take several round trips.
        let simulator = DeviceSimulator(model: model)
        var outbound = session.controller.start()
        while !outbound.isEmpty {
            _ = try? session.controller.feed(bytes: simulator.receive(bytes: outbound))
            outbound = session.controller.takeOutbound()
        }
        session.state = session.controller.state()
        if let info {
            session.state.info = info
        }
        session.connection = .connected
        return session
    }
}
#endif
