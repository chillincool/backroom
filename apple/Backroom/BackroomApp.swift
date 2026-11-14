import AppKit
import BackroomCore
import SwiftUI

@main
struct BackroomApp: App {
    private static let login = BrowserLogin()
    @State private var prefs = Preferences()
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
                .environment(prefs)
                .task {
                    prefs.applyAppearance()
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
        .commands { ChatCommands(model: model) }

        Settings {
            SettingsView()
                .environment(model)
                .environment(prefs)
        }
    }
}

/// File menu: join a channel, browse channels, message someone, on the
/// selected buffer's network.
struct ChatCommands: Commands {
    let model: AppModel

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            let network = model.phase == .online ? model.currentNetwork : nil
            Button("New Conversation…") { network.map { model.sheet = .newConversation(network: $0) } }
                .keyboardShortcut("n")
                .disabled(network == nil)
            Button("Join Channel…") { network.map { model.sheet = .joinChannel(network: $0) } }
                .keyboardShortcut("j")
                .disabled(network == nil)
            Button("Browse Channels…") { network.map { model.sheet = .browseChannels(network: $0) } }
                .keyboardShortcut("j", modifiers: [.command, .shift])
                .disabled(network == nil)
        }
    }
}
