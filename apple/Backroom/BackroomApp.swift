import AppKit
import BackroomCore
import SwiftUI
import UserNotifications

@main
struct BackroomApp: App {
    private static let login = BrowserLogin()
    private static let notifier = Notifier()
    @State private var prefs: Preferences
    @State private var model: AppModel

    init() {
        let prefs = Preferences()
        _prefs = State(initialValue: prefs)
        let model = AppModel(
            settings: ServerSettings.load(),
            authenticator: Self.login,
            preferences: prefs,
            makeClient: { settings in
                try BackroomClient(
                    config: settings.clientConfig,
                    secretStore: KeychainSecretStore(
                        service: "net.chillincool.backroom", account: settings.keychainAccount))
            })
        _model = State(initialValue: model)
        // Under XCTest there's no session, and no notifications.
        if ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil {
            let notifier = Self.notifier
            UNUserNotificationCenter.current().delegate = notifier
            notifier.open = { [weak model] id in model?.reveal(id) }
            model.notify = { [weak prefs] buffer, message in
                notifier.post(
                    message, in: buffer, network: model.networkName(buffer.id.network),
                    sound: prefs?.notificationSound ?? true)
            }
        }
    }

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
            Button("Search…") { network.map { model.sheet = .search(network: $0) } }
                .keyboardShortcut("f")
                .disabled(network == nil)
            Divider()
            Button("New Tab") { model.newTab() }
                .keyboardShortcut("t")
                .disabled(model.phase != .online)
            Button("Next Tab") { model.cycleTab(by: 1) }
                .keyboardShortcut("]", modifiers: [.command, .shift])
                .disabled(model.tabs.count < 2)
            Button("Previous Tab") { model.cycleTab(by: -1) }
                .keyboardShortcut("[", modifiers: [.command, .shift])
                .disabled(model.tabs.count < 2)
            Menu("Go to Tab") {
                ForEach(Array(model.tabs.prefix(9).enumerated()), id: \.element.id) { i, tab in
                    Button(model.tabTitle(tab)) { model.showTab(number: i + 1) }
                        .keyboardShortcut(KeyEquivalent(Character(String(i == 8 ? 9 : i + 1))))
                }
            }
            .disabled(model.tabs.count < 2)
            Divider()
            // Panes. Close Pane comes before the window's Close, so ⌘W closes
            // a pane while there are several and the window after that.
            let online = model.phase == .online
            Button("Split Right") { model.split(.horizontal) }
                .keyboardShortcut("d")
                .disabled(!online)
            Button("Split Down") { model.split(.vertical) }
                .keyboardShortcut("d", modifiers: [.command, .shift])
                .disabled(!online)
            Button(model.layout.panes.count > 1 ? "Close Pane" : "Close Tab") { model.closePane() }
                .keyboardShortcut("w")
                .disabled(!model.canClosePane)
            Button("Close Other Panes") { model.closeOtherPanes() }
                .keyboardShortcut("w", modifiers: [.command, .option])
                .disabled(model.layout.panes.count < 2)
            Menu("Move Focus") {
                Button("Left") { model.moveFocus(.left) }
                    .keyboardShortcut(.leftArrow, modifiers: [.command, .option])
                Button("Right") { model.moveFocus(.right) }
                    .keyboardShortcut(.rightArrow, modifiers: [.command, .option])
                Button("Up") { model.moveFocus(.up) }
                    .keyboardShortcut(.upArrow, modifiers: [.command, .option])
                Button("Down") { model.moveFocus(.down) }
                    .keyboardShortcut(.downArrow, modifiers: [.command, .option])
            }
            .disabled(model.layout.panes.count < 2)
        }
    }
}
