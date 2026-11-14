import AppKit
import BackroomCore
import SwiftUI

@main
struct BackroomApp: App {
    private static let login = BrowserLogin()
    @State private var model = AppModel(
        settings: ServerSettings.load(),
        authenticator: login,
        makeClient: { settings in
            try BackroomClient(
                config: settings.clientConfig,
                secretStore: KeychainSecretStore(
                    service: "net.chillincool.backroom", account: settings.keychainAccount))
        })

    var body: some Scene {
        Window("Backroom", id: "main") {
            ContentView()
                .environment(model)
                .task {
                    // Unit tests run inside the app; don't touch the Keychain
                    // or connect to soju then.
                    guard ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil
                    else { return }
                    await model.start()
                }
                // The login redirect (backroom://oauth/callback?code=…).
                .onOpenURL { Self.login.handle($0) }
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
                    model.isActive = true
                }
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.didResignActiveNotification)) { _ in
                    model.isActive = false
                }
        }
        .defaultSize(width: 1000, height: 650)

        Settings {
            SettingsView()
                .environment(model)
        }
    }
}
