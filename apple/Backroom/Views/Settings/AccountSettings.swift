import SwiftUI

/// The saved servers (one in use at a time) and the login.
struct AccountSettings: View {
    @Environment(AppModel.self) private var model
    @State private var editing: ServerSettings?

    var body: some View {
        Form {
            Section {
                ForEach(model.servers.servers) { server in
                    ServerRow(server: server, inUse: server.id == model.settings.id) {
                        editing = server
                    }
                }
                HStack {
                    Button("Add Server…") { editing = .blank }
                    #if DEBUG
                    Button("Add Test Servers") {
                        Task {
                            for server in [ServerSettings.e2e, .e2ePassword]
                            where !model.servers.servers.contains(where: { $0.id == server.id }) {
                                await model.save(server: server)
                            }
                        }
                    }
                    .help("The e2e stack: alice (single sign-on) and dave (password dave-secret)")
                    #endif
                }
            } header: {
                Text("Servers")
            } footer: {
                Text("Backroom uses one server at a time; pick it here or on the login screen. Each server keeps its own login.")
                    .foregroundStyle(.secondary)
            }
            Section("Login") {
                LabeledContent("Status") {
                    Text(status).foregroundStyle(.secondary)
                }
                if model.phase == .online || model.phase == .loggingIn {
                    HStack {
                        Spacer()
                        if model.phase == .online {
                            Button("Log Out", role: .destructive) { model.logOut() }
                        } else {
                            Button("Cancel Login") { model.cancelLogin() }
                        }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .sheet(item: $editing) { server in
            ServerEditor(
                server: server,
                isNew: !model.servers.servers.contains { $0.id == server.id },
                save: { saved in Task { await model.save(server: saved) } },
                remove: { Task { await model.remove(server: server.id) } })
        }
        .onChange(of: model.serverToEdit, initial: true) {
            guard let server = model.serverToEdit else { return }
            model.serverToEdit = nil
            editing = server
        }
    }

    private var status: String {
        switch model.phase {
        case .online: model.control?.summary ?? "Connecting…"
        case .loggingIn: "Finish logging in in your browser"
        case .loggedOut: model.servers.servers.isEmpty ? "No server yet" : "Logged out; log in from the main window"
        case .failed(let message): "Couldn't connect: \(message)"
        }
    }
}

private struct ServerRow: View {
    @Environment(AppModel.self) private var model
    let server: ServerSettings
    let inUse: Bool
    let edit: () -> Void

    var body: some View {
        HStack {
            Image(systemName: inUse ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(inUse ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
            VStack(alignment: .leading, spacing: 2) {
                Text(server.displayName)
                Text("\(server.serverURL) · \(method)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if !inUse {
                Button("Use") { Task { await model.select(server: server.id) } }
            }
            Button("Edit…", action: edit)
        }
    }

    private var method: String {
        switch server.login {
        case .password: server.username.isEmpty ? "password" : "password as \(server.username)"
        case .singleSignOn: "single sign-on"
        }
    }
}

/// Adds or edits one server.
private struct ServerEditor: View {
    @Environment(\.dismiss) private var dismiss
    @State private var draft: ServerSettings
    @State private var confirmRemove = false
    let isNew: Bool
    let save: (ServerSettings) -> Void
    let remove: () -> Void

    init(server: ServerSettings, isNew: Bool, save: @escaping (ServerSettings) -> Void,
         remove: @escaping () -> Void) {
        _draft = State(initialValue: server)
        self.isNew = isNew
        self.save = save
        self.remove = remove
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("soju URL", text: $draft.serverURL, prompt: Text("wss://irc.example.net/socket"))
                    TextField("Name", text: $draft.name, prompt: Text(draft.serverURL.isEmpty ? "Optional" : draft.displayName))
                    Picker("Log in with", selection: $draft.login) {
                        Text("Username & password").tag(ServerSettings.Login.password)
                        Text("Single sign-on (OIDC)").tag(ServerSettings.Login.singleSignOn)
                    }
                    switch draft.login {
                    case .password:
                        TextField("Username", text: $draft.username, prompt: Text("Your soju username"))
                    case .singleSignOn:
                        TextField("Issuer", text: $draft.issuer, prompt: Text("https://login.example.net/realms/main"))
                    }
                    TextField("Nick", text: $draft.nick, prompt: Text(draft.login == .password ? "Same as username" : "Optional"))
                } footer: {
                    Text(draft.login == .password
                         ? "For a soju with its own accounts (the default)."
                         : "For a soju using `auth oauth2`. The issuer is your OIDC provider: a Keycloak realm, Authentik, …")
                        .foregroundStyle(.secondary)
                }
            }
            .formStyle(.grouped)
            HStack {
                if !isNew {
                    Button("Remove Server…", role: .destructive) { confirmRemove = true }
                }
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(isNew ? "Add" : "Save") {
                    draft.serverURL = draft.serverURL.trimmingCharacters(in: .whitespaces)
                    save(draft)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!draft.isComplete)
            }
            .padding()
        }
        .frame(width: 480)
        .confirmationDialog("Remove \(draft.displayName)?", isPresented: $confirmRemove) {
            Button("Remove", role: .destructive) {
                remove()
                dismiss()
            }
        } message: {
            Text("Its saved login is deleted too. Your networks and history stay on the server.")
        }
    }
}
