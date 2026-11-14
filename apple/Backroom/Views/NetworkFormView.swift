import BackroomCore
import SwiftUI

/// Adds a network to soju, or edits one (`editing`).
struct NetworkFormView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var editing: Network?
    @State private var name = ""
    @State private var host = ""
    @State private var port = 6697
    @State private var tls = true
    @State private var nick = ""

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
                TextField("Nick", text: $nick, prompt: Text("Your usual nick (optional)"))
            } footer: {
                Text("soju connects to the network and stays connected while you're away.")
                    .foregroundStyle(.secondary)
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
        }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(editing == nil ? "Add" : "Save") {
                    let fields = NetworkFields(
                        name: name.trimmingCharacters(in: .whitespaces),
                        host: host.trimmingCharacters(in: .whitespaces), port: port, tls: tls,
                        nick: nick.trimmingCharacters(in: .whitespaces))
                    if let network = editing {
                        model.changeNetwork(network.id, fields)
                    } else {
                        model.addNetwork(fields)
                    }
                    dismiss()
                }
                .disabled(host.trimmingCharacters(in: .whitespaces).isEmpty || !(1...65535).contains(port))
            }
        }
    }
}
