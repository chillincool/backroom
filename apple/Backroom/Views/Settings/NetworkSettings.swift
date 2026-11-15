import BackroomCore
import SwiftUI

/// The networks (soju's, or this Mac's without a bouncer): add, edit, remove.
struct NetworkSettings: View {
    @Environment(AppModel.self) private var model
    @State private var selection: String?
    @State private var adding = false
    @State private var editing: Network?
    @State private var removing: Network?

    var body: some View {
        Form {
            if model.phase != .online {
                ContentUnavailableView(
                    model.isStandalone ? "Not Connected" : "Not Logged In", systemImage: "network.slash",
                    description: Text(model.isStandalone
                                      ? "Connect from the main window to manage your networks."
                                      : "Log in to manage your networks."))
            } else {
                Section {
                    List(selection: $selection) {
                        ForEach(model.networks) { network in
                            NetworkSettingsRow(network: network)
                                .tag(network.id)
                                .contextMenu {
                                    Button("Edit…") { editing = network.network }
                                    Button("Remove…", role: .destructive) { removing = network.network }
                                }
                        }
                    }
                    .frame(minHeight: 200)
                    .overlay {
                        if model.networks.isEmpty { Text("No networks").foregroundStyle(.secondary) }
                    }
                } footer: {
                    HStack {
                        Button {
                            adding = true
                        } label: { Image(systemName: "plus") }
                            .help("Add a network")
                        Button {
                            removing = selected
                        } label: { Image(systemName: "minus") }
                            .help("Remove the selected network")
                            .disabled(selected == nil)
                        Spacer()
                        Button("Edit…") { editing = selected }
                            .disabled(selected == nil)
                    }
                    .buttonStyle(.borderless)
                }
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $adding) {
            NavigationStack { NetworkFormView() }
        }
        .sheet(item: $editing) { network in
            NavigationStack { NetworkFormView(editing: network) }
        }
        .confirmationDialog(
            "Remove \(removing?.name ?? "")?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button("Remove Network", role: .destructive) {
                if let removing { model.removeNetwork(removing.id) }
            }
        } message: {
            Text(model.removeNetworkMessage)
        }
    }

    private var selected: Network? {
        model.networks.first { $0.id == selection }?.network
    }
}

extension Network: @retroactive Identifiable {}

struct NetworkSettingsRow: View {
    let network: NetworkState

    var body: some View {
        let a = network.network.attributes
        HStack {
            Circle()
                .fill(network.connection.isConnected ? Color.green : Color.orange)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading) {
                Text(network.network.name).fontWeight(.medium)
                Text([a["host"].map { "\($0):\(a["port"] ?? "")" }, a["nickname"].map { "as \($0)" }]
                    .compactMap { $0 }.joined(separator: " "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text(network.connection.summary)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.vertical, 2)
    }
}
