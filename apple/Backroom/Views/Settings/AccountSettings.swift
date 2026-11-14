import SwiftUI

/// Which soju to use, how to log in, and the login itself.
struct AccountSettings: View {
    @Environment(AppModel.self) private var model
    @State private var draft = ServerSettings.e2e

    var body: some View {
        Form {
            Section("Server") {
                TextField("soju URL", text: $draft.serverURL, prompt: Text("wss://irc.example.net/socket"))
                Picker("Log in with", selection: $draft.login) {
                    Text("Username & password").tag(ServerSettings.Login.password)
                    Text("Single sign-on (OIDC)").tag(ServerSettings.Login.singleSignOn)
                }
                switch draft.login {
                case .password:
                    TextField("Username", text: $draft.username, prompt: Text("Your soju username"))
                case .singleSignOn:
                    TextField("Login issuer", text: $draft.issuer, prompt: Text("https://login.example.net/realms/main"))
                }
                TextField("Nick", text: $draft.nick, prompt: Text(draft.login == .password ? "Same as username" : ""))
                HStack {
                    Menu("Use Local Test Stack") {
                        Button("Single sign-on (alice)") { draft = .e2e }
                        Button("Password (dave)") { draft = .e2ePassword }
                    }
                    .fixedSize()
                    Spacer()
                    Button("Save and Reconnect") {
                        Task { await model.apply(settings: draft) }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(draft == model.settings || !draft.isComplete)
                }
            }
            Section {
                LabeledContent("Status") {
                    Text(status).foregroundStyle(.secondary)
                }
                HStack {
                    Spacer()
                    switch model.phase {
                    case .online:
                        Button("Log Out", role: .destructive) { model.logOut() }
                    case .loggingIn:
                        Button("Cancel Login") { model.cancelLogin() }
                    case .loggedOut, .failed:
                        // A password goes in the main window.
                        if model.settings.login == .singleSignOn {
                            Button("Log In") { Task { await model.logIn() } }
                        }
                    }
                }
            } header: {
                Text("Login")
            } footer: {
                Text("Changing the server signs you in again; each server keeps its own login.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear { draft = model.settings }
    }

    private var status: String {
        switch model.phase {
        case .online: model.control?.summary ?? "Connecting…"
        case .loggingIn: "Finish logging in in your browser"
        case .loggedOut where model.settings.login == .password: "Logged out; enter your password in the main window"
        case .loggedOut: "Logged out"
        case .failed(let message): "Couldn't connect: \(message)"
        }
    }
}
