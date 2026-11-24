import BarBudsCore
import Foundation
import Observation

/// Tracks every supported pair of earbuds connected to the Mac. It opens a
/// session to each one as it appears and closes it when the device goes away.
@Observable
final class DeviceManager {
    private(set) var sessions: [EarbudsSession] = []
    /// The device shown in the popover. Falls back to the first session.
    var selectedID: EarbudsSession.ID?

    @ObservationIgnored private var discoveryTask: Task<Void, Never>?

    /// Starts discovery immediately, so devices are ready before the popover opens.
    init(discovery: any DeviceDiscovery) {
        let events = discovery.events()
        discoveryTask = Task { [weak self] in
            for await event in events {
                self?.handle(event)
            }
        }
    }

    var selected: EarbudsSession? {
        sessions.first { $0.id == selectedID } ?? sessions.first
    }

    private func handle(_ event: DiscoveryEvent) {
        switch event {
        case .connected(let device):
            guard !sessions.contains(where: { $0.id == device.id }) else { return }
            let session = EarbudsSession(device: device)
            sessions.append(session)
            Task { await session.connect() }

        case .disconnected(let id):
            guard let index = sessions.firstIndex(where: { $0.id == id }) else { return }
            let session = sessions.remove(at: index)
            Task { await session.disconnect() }
        }
    }
}

#if DEBUG
extension DeviceManager {
    /// A manager showing the given sessions, for previews. Runs no discovery.
    convenience init(previewSessions: [EarbudsSession]) {
        self.init(discovery: MockDiscovery(names: []))
        sessions = previewSessions
    }

    /// Simulates a pair of `model` earbuds connecting to the Mac. It runs
    /// the model's real driver against a simulator of its protocol.
    func simulateConnect(_ model: DeviceModel) {
        let displayName = BarBudsController(model: model).capabilities().displayName
        let sameModel = sessions.filter { $0.capabilities.displayName == displayName }.count
        let name = "Simulated \(displayName)" + (sameModel > 0 ? " \(sameModel + 1)" : "")
        let device = DiscoveredDevice(
            id: "simulated-\(UUID().uuidString)",
            name: name,
            model: model,
            makeTransport: { SimulatedTransport(model: model) }
        )
        handle(.connected(device))
        selectedID = device.id
    }

    /// Simulates the selected earbuds disconnecting from the Mac.
    func simulateDisconnect() {
        guard let id = selected?.id else { return }
        handle(.disconnected(id: id))
    }
}
#endif
