import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var draft = ServerSettings.e2e

    var body: some View {
        Form {
            Section {
                TextField("soju URL", text: $draft.serverURL, prompt: Text("wss://irc.example.net/socket"))
                TextField("Login issuer", text: $draft.issuer, prompt: Text("https://login.example.net/realms/main"))
                TextField("Nick", text: $draft.nick)
            } footer: {
                Text("Changing the server signs you in again; each server keeps its own login.")
                    .foregroundStyle(.secondary)
            }
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
        .formStyle(.grouped)
        .frame(width: 480)
        .onAppear { draft = model.settings }
    }
}
