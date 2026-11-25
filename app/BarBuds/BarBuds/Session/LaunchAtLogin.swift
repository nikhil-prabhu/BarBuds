import Observation
import OSLog
import ServiceManagement

/// Whether BarBuds opens at login, as a login item registered with
/// `SMAppService` (listed in System Settings › General › Login Items).
@Observable
final class LaunchAtLogin {
    static let shared = LaunchAtLogin()

    private static let log = Logger(subsystem: "com.nikhilprabhu.BarBuds", category: "login-item")

    private(set) var status = SMAppService.mainApp.status

    /// Registered and allowed to run.
    var isEnabled: Bool { status == .enabled }

    /// Registered, but switched off in System Settings: only the user can
    /// turn it back on there.
    var needsApproval: Bool { status == .requiresApproval }

    /// Reads the status again, e.g. after the user changed it in System Settings.
    func refresh() {
        status = SMAppService.mainApp.status
    }

    func setEnabled(_ enabled: Bool) {
        if enabled, needsApproval {
            SMAppService.openSystemSettingsLoginItems()
            return
        }
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            Self.log.error("Changing the login item failed: \(error.localizedDescription, privacy: .public)")
        }
        refresh()
        if enabled, needsApproval {
            SMAppService.openSystemSettingsLoginItems()
        }
    }
}
