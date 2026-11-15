import AppKit
import BackroomCore
import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model
    @Environment(Preferences.self) private var prefs
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Group {
            switch model.phase {
            case .online:
                MainView()
            case .loggedOut, .loggingIn, .failed:
                LoginView()
            }
        }
        .safeAreaInset(edge: .bottom) {
            if let notice = model.notice {
                NoticeBar(text: notice) { model.notice = nil }
            }
        }
        .environment(\.theme, prefs.theme(dark: colorScheme == .dark))
        .onChange(of: prefs.highlightWords) { model.refreshMentions() }
        // Unread mentions and private messages on the Dock icon.
        .onChange(of: model.totalMentions, initial: true) { _, count in
            NSApp?.dockTile.badgeLabel = count > 0 ? "\(count)" : nil
        }
        .onChange(of: prefs.ignored) { model.refreshMentions() }
        .onChange(of: prefs.activeTextLogPath) { model.applyTextLogs() }
    }
}

struct NoticeBar: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
            Text(text).textSelection(.enabled)
            Spacer()
            Button("Dismiss", action: dismiss)
        }
        .padding(10)
        .background(.bar)
    }
}

/// The first screen: pick a saved server and log in the way it's set up
/// (a password, or single sign-on). Servers are added and edited in
/// Settings → Account.
struct LoginView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openSettings) private var openSettings
    @State private var password = ""
    @FocusState private var passwordFocused: Bool

    private var server: ServerSettings { model.settings }

    var body: some View {
        VStack(spacing: 20) {
            VStack(spacing: 8) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 48))
                    .foregroundStyle(.secondary)
                Text("Backroom").font(.largeTitle.bold())
            }

            if case .failed(let message) = model.phase {
                Text("Couldn't connect: \(message)")
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
            Form {
                Picker("Server", selection: serverSelection) {
                    ForEach(model.servers.bouncers) { Text($0.displayName).tag($0.id) }
                    if !model.servers.bouncers.isEmpty { Divider() }
                    Text(model.servers.standalone.displayName).tag(model.servers.standalone.id)
                }
                if server.login == .password && server.isComplete {
                    LabeledContent("Username", value: server.username)
                    SecureField("Password", text: $password)
                        .textContentType(.password)
                        .focused($passwordFocused)
                        .onSubmit(logIn)
                }
            }
            .frame(width: 360)
            .fixedSize(horizontal: false, vertical: true)
            .disabled(model.phase == .loggingIn)

            loginButtons

            Button("Add soju Server…", action: addServer)
                .buttonStyle(.link)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { passwordFocused = true }
        .onChange(of: server.id) {
            password = ""
            passwordFocused = true
        }
    }

    private var serverSelection: Binding<UUID> {
        Binding(get: { server.id }, set: { id in Task { await model.select(server: id) } })
    }

    @ViewBuilder private var loginButtons: some View {
        if server.isStandalone {
            Button("Connect") { Task { await model.connectStandalone() } }
                .keyboardShortcut(.defaultAction)
                .controlSize(.large)
            Text("Connects to your networks directly, without a bouncer.\nHistory is kept on this Mac.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        } else if !server.isComplete {
            Text(server.login == .singleSignOn
                 ? "This server needs a single sign-on issuer."
                 : "This server needs a username.")
                .foregroundStyle(.secondary)
            Button("Edit Server…", action: editServer)
        } else if model.phase == .loggingIn {
            ProgressView().controlSize(.small)
            Text("Finish logging in in your browser.")
                .foregroundStyle(.secondary)
            Button("Cancel") { model.cancelLogin() }
        } else {
            switch server.login {
            case .password:
                Button("Log In", action: logIn)
                    .keyboardShortcut(.defaultAction)
                    .controlSize(.large)
                    .disabled(password.isEmpty)
            case .singleSignOn:
                Button("Log In with Single Sign-On", action: logIn)
                    .keyboardShortcut(.defaultAction)
                    .controlSize(.large)
                Text(server.issuer)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            case .standalone:
                EmptyView()
            }
        }
    }

    private func logIn() {
        switch server.login {
        case .password:
            guard !password.isEmpty else { return }
            let entered = password
            password = ""
            Task { await model.logIn(password: entered) }
        case .singleSignOn:
            Task { await model.logIn() }
        case .standalone:
            Task { await model.connectStandalone() }
        }
    }

    private func addServer() { open(.blank) }
    private func editServer() { open(server) }

    /// Opens Settings → Account with `server` in the editor.
    private func open(_ server: ServerSettings) {
        model.serverToEdit = server
        UserDefaults.standard.set(SettingsView.Tab.account.rawValue, forKey: "settingsTab")
        openSettings()
    }
}
