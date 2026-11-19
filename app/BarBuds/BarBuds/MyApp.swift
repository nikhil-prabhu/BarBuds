import SwiftUI

@main struct BarBudsApp: App {
    var body: some Scene {
        MenuBarExtra("BarBuds", systemImage: "earbuds") {
            PlaceHolderView()
        }
        .menuBarExtraStyle(.window)
        
        Window("About BarBuds", id: "about") {
            AboutView()
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)
    }
}

struct PlaceHolderView: View {
    @Environment(\.openWindow) private var openWindow
    
    var body: some View {
        VStack(spacing: 12) {
            HStack {
                Spacer()
                Menu {
                    Button("About BarBuds") {
                        openAboutWindow()
                    }
                    Divider()
                    Button("Quit BarBuds") {
                        NSApplication.shared.terminate(nil)
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .foregroundStyle(.secondary)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            
            HStack(spacing: 8) {
                Image(systemName: "earbuds")
                    .font(.title2)
                Text("Hello world!")
                    .font(.headline)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
        .padding(16)
        .frame(width: 240)
    }
    
    private func openAboutWindow() {
        NSApp.activate()
        openWindow(id: "about")
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
