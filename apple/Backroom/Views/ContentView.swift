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

/// The first screen: which soju, and the login. A username and password by
/// default; single sign-on (OIDC, for `auth oauth2`) with the issuer set in
/// Settings → Account.
struct LoginView: View {
    @Environment(AppModel.self) private var model
    @State private var draft = ServerSettings.blank
    @State private var password = ""
    @FocusState private var focus: Field?

    private enum Field { case server, username, password }

    var body: some View {
        VStack(spacing: 20) {
            VStack(spacing: 8) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 48))
                    .foregroundStyle(.secondary)
                Text("Backroom").font(.largeTitle.bold())
                Text("Log in to your soju bouncer").foregroundStyle(.secondary)
            }

            if case .failed(let message) = model.phase {
                Text("Couldn't connect: \(message)")
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }

            Form {
                TextField("Server", text: $draft.serverURL, prompt: Text("wss://irc.example.net/socket"))
                    .focused($focus, equals: .server)
                if draft.login == .password {
                    TextField("Username", text: $draft.username)
                        .textContentType(.username)
                        .focused($focus, equals: .username)
                    SecureField("Password", text: $password)
                        .textContentType(.password)
                        .focused($focus, equals: .password)
                        .onSubmit(logIn)
                }
            }
            .frame(width: 360)
            .fixedSize(horizontal: false, vertical: true)
            .disabled(model.phase == .loggingIn)

            switch draft.login {
            case .password: passwordButtons
            case .singleSignOn: singleSignOnButtons
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            draft = model.settings
            focus = draft.serverURL.isEmpty ? .server
                : draft.login == .password ? (draft.username.isEmpty ? .username : .password) : nil
        }
    }

    /// The issuer comes from Settings → Account, read when it's used (the
    /// Settings window can change it while this screen is open).
    private var issuer: String { model.settings.issuer }

    @ViewBuilder private var passwordButtons: some View {
        Button("Log In", action: logIn)
            .keyboardShortcut(.defaultAction)
            .controlSize(.large)
            .disabled(!draft.isComplete || password.isEmpty)

        VStack(spacing: 8) {
            HStack {
                VStack { Divider() }
                Text("or").foregroundStyle(.secondary)
                VStack { Divider() }
            }
            .frame(width: 260)
            if issuer.isEmpty {
                SettingsLink { Text("Set Up Single Sign-On…") }
                Text("OIDC (Keycloak, Authentik, …), in Settings → Account")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Button("Log In with Single Sign-On") {
                    draft.login = .singleSignOn
                    logIn()
                }
                .disabled(draft.serverURL.isEmpty)
                Text(issuer)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder private var singleSignOnButtons: some View {
        if model.phase == .loggingIn {
            ProgressView().controlSize(.small)
            Text("Finish logging in in your browser.")
                .foregroundStyle(.secondary)
            Button("Cancel") { model.cancelLogin() }
        } else {
            if issuer.isEmpty {
                SettingsLink { Text("Set Up Single Sign-On…") }
                    .controlSize(.large)
            } else {
                Button("Log In with Single Sign-On", action: logIn)
                    .keyboardShortcut(.defaultAction)
                    .controlSize(.large)
                    .disabled(draft.serverURL.isEmpty)
            }
            Button("Use a Username and Password Instead") {
                draft.login = .password
                focus = draft.username.isEmpty ? .username : .password
            }
            .buttonStyle(.link)
        }
    }

    private func logIn() {
        draft.serverURL = draft.serverURL.trimmingCharacters(in: .whitespaces)
        draft.issuer = issuer
        guard draft.isComplete else { return }
        switch draft.login {
        case .password:
            guard !password.isEmpty else { return }
            let entered = password
            password = ""
            Task { await model.logIn(draft, password: entered) }
        case .singleSignOn:
            Task { await model.logIn(draft) }
        }
    }
}
