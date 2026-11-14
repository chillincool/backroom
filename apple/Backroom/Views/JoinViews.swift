import BackroomCore
import SwiftUI

/// Shows whichever sheet `AppModel.sheet` asks for.
struct SheetContent: View {
    @Environment(AppModel.self) private var model
    let sheet: AppModel.Sheet

    var body: some View {
        NavigationStack {
            switch sheet {
            case .joinChannel(let network): JoinChannelSheet(network: network)
            case .browseChannels(let network): ChannelBrowser(network: network)
            case .newConversation(let network): NewConversationSheet(network: network)
            case .whois(let network, let info): WhoisView(network: network, info: info)
            }
        }
    }
}

extension AppModel {
    func networkName(_ id: String) -> String {
        networks.first { $0.id == id }?.network.name ?? id
    }
}

struct JoinChannelSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    @State private var channel = ""
    @State private var key = ""

    var body: some View {
        Form {
            Section {
                TextField("Channel", text: $channel, prompt: Text("#channel"))
                SecureField("Key", text: $key, prompt: Text("Only for channels with a key"))
            } footer: {
                Text("Several channels can be joined at once: #one,#two.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 400)
        .navigationTitle("Join Channel on \(model.networkName(network))")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .principal) {
                Button("Browse…") { model.sheet = .browseChannels(network: network) }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Join") {
                    let key = key.trimmingCharacters(in: .whitespaces)
                    if model.join(network: network, channel: name, key: key.isEmpty ? nil : key) {
                        dismiss()
                    }
                }
                .disabled(name.isEmpty || name.contains(" ") || key.contains(" "))
            }
        }
    }

    private var name: String {
        let trimmed = channel.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty ? "" : Composer.withHashes(trimmed)
    }
}

/// The network's channel list (`LIST`), searchable and sortable.
struct ChannelBrowser: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    @State private var search = ""
    @State private var selection: ChannelListing.ID?
    @State private var sortOrder = [KeyPathComparator(\ChannelListing.users, order: .reverse)]

    var body: some View {
        let list = model.channelLists[network]
        let rows = filtered(list?.channels ?? [])
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Channel", value: \.name) { Text($0.name).lineLimit(1) }
                .width(min: 120, ideal: 180)
            TableColumn("Users", value: \.users) { Text($0.users, format: .number).monospacedDigit() }
                .width(min: 50, ideal: 60, max: 80)
            TableColumn("Topic", value: \.topic) { channel in
                let topic = stripFormatting(text: channel.topic)
                Text(topic).lineLimit(1).help(topic)
            }
        }
        .contextMenu(forSelectionType: ChannelListing.ID.self) { ids in
            if let id = ids.first { Button("Join \(id)") { join(id) } }
        } primaryAction: { ids in
            if let id = ids.first { join(id) }
        }
        .searchable(text: $search, placement: .toolbar, prompt: "Name or topic")
        .safeAreaInset(edge: .bottom) {
            HStack {
                if list?.loading ?? true {
                    ProgressView().controlSize(.small)
                    Text("Loading… \(list?.channels.count ?? 0) channels so far")
                } else {
                    Text(rows.count == list?.channels.count
                         ? "\(rows.count) channels" : "\(rows.count) of \(list?.channels.count ?? 0) channels")
                }
                Spacer()
                Button("Refresh") { model.listChannels(network: network) }
                    .disabled(list?.loading ?? true)
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(10)
            .background(.bar)
        }
        .frame(minWidth: 640, minHeight: 420)
        .navigationTitle("Channels on \(model.networkName(network))")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Join") { if let selection { join(selection) } }
                    .disabled(selection == nil)
            }
        }
        .onAppear {
            // Reuse a finished list; big networks take a while to send one.
            if model.channelLists[network] == nil { model.listChannels(network: network) }
        }
    }

    private func filtered(_ channels: [ChannelListing]) -> [ChannelListing] {
        let query = search.trimmingCharacters(in: .whitespaces)
        let matching = query.isEmpty ? channels : channels.filter {
            $0.name.localizedCaseInsensitiveContains(query) || $0.topic.localizedCaseInsensitiveContains(query)
        }
        return matching.sorted(using: sortOrder)
    }

    private func join(_ name: String) {
        if model.join(network: network, channel: name) { dismiss() }
    }
}

extension ChannelListing: @retroactive Identifiable {
    public var id: String { name }
}

struct NewConversationSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    @State private var nick = ""

    var body: some View {
        Form {
            TextField("Nick", text: $nick, prompt: Text("Who to message"))
        }
        .formStyle(.grouped)
        .frame(width: 360)
        .navigationTitle("New Conversation on \(model.networkName(network))")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Open") {
                    model.openQuery(network: network, nick: trimmed)
                    dismiss()
                }
                .disabled(trimmed.isEmpty || trimmed.contains(" ") || "#&+!".contains(trimmed.first!))
            }
        }
    }

    private var trimmed: String { nick.trimmingCharacters(in: .whitespaces) }
}
