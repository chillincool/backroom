import BackroomCore
import SwiftUI

/// Adds a network, or edits one (`editing`). With soju, soju keeps the
/// network; without a bouncer, Backroom does and the form also has the
/// network login (SASL, server password or NickServ).
struct NetworkFormView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var editing: Network?
    @State private var name = ""
    @State private var host = ""
    @State private var port = 6697
    @State private var tls = true
    @State private var nick = ""
    @State private var login = NetworkLogin()
    /// The login saved on the network being edited.
    @State private var savedLogin: NetworkLogin?

    var body: some View {
        Form {
            Section {
                TextField("Server", text: $host, prompt: Text("irc.libera.chat"))
                TextField("Port", value: $port, format: .number.grouping(.never))
                Toggle("Use TLS", isOn: $tls)
                    .onChange(of: tls) { _, on in
                        // Follow the usual ports unless the user picked another.
                        if on, port == 6667 { port = 6697 } else if !on, port == 6697 { port = 6667 }
                    }
            }
            Section {
                TextField("Name", text: $name, prompt: Text("Shown in the sidebar (optional)"))
                TextField("Nick", text: $nick, prompt: Text(nickPrompt))
            } footer: {
                if !model.isStandalone {
                    Text("soju connects to the network and stays connected while you're away.")
                        .foregroundStyle(.secondary)
                }
            }
            if model.isStandalone {
                loginSection
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .navigationTitle(editing == nil ? "Add Network" : "Edit \(editing!.name)")
        .onAppear {
            guard let network = editing else { return }
            let a = network.attributes
            name = a["name"] ?? ""
            host = a["host"] ?? ""
            tls = a["tls"] != "0"
            port = a["port"].flatMap(Int.init) ?? (tls ? 6697 : 6667)
            nick = a["nickname"] ?? ""
            login = NetworkLogin(attributes: a)
            savedLogin = login
        }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(editing == nil ? "Add" : "Save", action: save)
                    .disabled(!canSave)
            }
        }
    }

    private var nickPrompt: String {
        guard model.isStandalone else { return "Your usual nick (optional)" }
        let fallback = model.settings.clientConfig().nick
        return "\(fallback) (from Settings → Account)"
    }

    @ViewBuilder private var loginSection: some View {
        Section {
            Picker("Log in with", selection: $login.method) {
                ForEach(NetworkLogin.Method.allCases) { Text($0.title).tag($0) }
            }
            if login.method == .sasl {
                TextField("Account", text: $login.username, prompt: Text(nick.isEmpty ? "Same as nick" : nick))
            }
            if login.method != .none {
                SecureField("Password", text: $login.password, prompt: Text(passwordPrompt))
            }
        } header: {
            Text("Login")
        } footer: {
            Text(loginHelp).foregroundStyle(.secondary)
        }
    }

    private var passwordPrompt: String {
        savedLogin?.method == login.method && login.method != .none ? "Saved; type to change" : "Required"
    }

    private var loginHelp: String {
        switch login.method {
        case .none: "For networks without an account, or to log in later with /msg NickServ."
        case .sasl: "Logs in to your account while connecting. Most networks (Libera, OFTC) support it."
        case .serverPassword: "Sent before anything else, for servers that need a password to connect."
        case .nickServ: "Sent to NickServ after connecting, for networks without SASL."
        }
    }

    private var canSave: Bool {
        !host.trimmingCharacters(in: .whitespaces).isEmpty && (1...65535).contains(port)
            && (!model.isStandalone || login.isComplete(saved: savedLogin))
    }

    private func save() {
        var login = login
        login.username = login.username.trimmingCharacters(in: .whitespaces)
        let fields = NetworkFields(
            name: name.trimmingCharacters(in: .whitespaces),
            host: host.trimmingCharacters(in: .whitespaces), port: port, tls: tls,
            nick: nick.trimmingCharacters(in: .whitespaces),
            login: model.isStandalone ? login : nil)
        if let network = editing {
            model.changeNetwork(network.id, fields)
        } else {
            model.addNetwork(fields)
        }
        dismiss()
    }
}
