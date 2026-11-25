import SwiftUI

@main struct BarBudsApp: App {
    @NSApplicationDelegateAdaptor private var appDelegate: AppDelegate

    /// Xcode runs the whole app to render previews. Without this check, every
    /// preview session adds a second, inert BarBuds icon to the menu bar.
    private static let isRunningForPreviews =
        ProcessInfo.processInfo.environment["XCODE_RUNNING_FOR_PREVIEWS"] == "1"

    @State private var devices = DeviceManager(
        discovery: isRunningForPreviews ? NoDiscovery() as any DeviceDiscovery : BluetoothDiscovery()
    )

    var body: some Scene {
        MenuBarExtra("BarBuds", systemImage: "earbuds", isInserted: .constant(!Self.isRunningForPreviews)) {
            MenuContentView(devices: devices)
        }
        .menuBarExtraStyle(.window)
        
        // Retitled for headphones by DeviceInfoView.
        Window("About These Earbuds", id: "device-info") {
            DeviceInfoView(devices: devices)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Window("About BarBuds", id: "about") {
            AboutView()
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    /// A menu bar utility has no main window. Without this, AppKit quits the app
    /// whenever the menu bar panel or the About window is the last window to close.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}

struct AboutView: View {
    private var versionString: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "Version \(version) (\(build))"
    }
    
    var body: some View {
        VStack(spacing: 12) {
            Text("🍻")
                .font(.system(size: 56))
            
            VStack(spacing: 4) {
                Text("BarBuds")
                    .font(.title2.bold())
                Text(versionString)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            
            Text("A macOS menu bar utility to configure wireless earbuds.")
                .font(.callout)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 8)
            
            Divider()
                .padding(.vertical, 4)
            
            VStack(spacing: 4) {
                Text("Copyright © 2026 Nikhil Prabhu")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                
                Link(
                    "Licensed under the GNU GPL-3.0",
                    destination: URL(string: "https://www.gnu.org/licenses/gpl-3.0.html")!
                )
                .font(.caption)
            }
        }
        .padding(24)
        .frame(width: 280)
    }
}
