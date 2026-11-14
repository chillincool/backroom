import SwiftUI

/// Which soju and login server to use, and the login itself.
struct AccountSettings: View {
    @Environment(AppModel.self) private var model
    @State private var draft = ServerSettings.e2e

    var body: some View {
        Form {
            Section("Server") {
                TextField("soju URL", text: $draft.serverURL, prompt: Text("wss://irc.example.net/socket"))
                TextField("Login issuer", text: $draft.issuer, prompt: Text("https://login.example.net/realms/main"))
                TextField("Nick", text: $draft.nick)
                HStack {
                    Button("Use Local Test Stack") { draft = .e2e }
                    Spacer()
                    Button("Save and Reconnect") {
                        Task { await model.apply(settings: draft) }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(draft == model.settings || draft.serverURL.isEmpty || draft.issuer.isEmpty)
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
                        Button("Log In") { Task { await model.logIn() } }
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
        case .loggedOut: "Logged out"
        case .failed(let message): "Couldn't connect: \(message)"
        }
    }
}
