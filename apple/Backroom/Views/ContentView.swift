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

struct LoginView: View {
    @Environment(AppModel.self) private var model
    @State private var password = ""

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text("Backroom").font(.largeTitle.bold())
            Text(model.settings.serverURL)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)

            if case .failed(let message) = model.phase {
                Text("Couldn't connect: \(message)")
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                Button("Try Again") { Task { await model.start() } }
            }

            switch model.settings.login {
            case .password: passwordLogin
            case .singleSignOn: singleSignOn
            }

            SettingsLink { Text("Server Settings…") }
                .buttonStyle(.link)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder private var passwordLogin: some View {
        if model.settings.username.isEmpty {
            Text("Set your soju username in Server Settings.")
                .foregroundStyle(.secondary)
        } else {
            Form {
                LabeledContent("Username", value: model.settings.username)
                SecureField("Password", text: $password)
                    .onSubmit(logIn)
            }
            .frame(maxWidth: 300)
            .fixedSize(horizontal: false, vertical: true)
            Button("Log In", action: logIn)
                .keyboardShortcut(.defaultAction)
                .controlSize(.large)
                .disabled(password.isEmpty)
        }
    }

    private func logIn() {
        let entered = password
        password = ""
        Task { await model.logIn(password: entered) }
    }

    @ViewBuilder private var singleSignOn: some View {
        Button {
            Task { await model.logIn() }
        } label: {
            if model.phase == .loggingIn {
                ProgressView().controlSize(.small)
            } else {
                Text("Log In")
            }
        }
        .keyboardShortcut(.defaultAction)
        .controlSize(.large)
        .disabled(model.phase == .loggingIn)

        if model.phase == .loggingIn {
            Text("Finish logging in in your browser.")
                .foregroundStyle(.secondary)
            Button("Cancel") { model.cancelLogin() }
        }
    }
}
