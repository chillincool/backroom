import BackroomCore
import SwiftUI

struct MainView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("showMembers") private var showMembers = true

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 180, ideal: 220)
        } detail: {
            Group {
                if let id = model.selection, model.buffers[id] != nil {
                    BufferView(id: id)
                        .id(id)
                } else {
                    ContentUnavailableView("No Conversation Selected", systemImage: "bubble.left")
                }
            }
            // Only channels have members; elsewhere the panel stays shut
            // without forgetting that it's wanted.
            .inspector(isPresented: Binding(
                get: { showMembers && model.selectedBuffer?.kind == .channel },
                set: { if model.selectedBuffer?.kind == .channel { showMembers = $0 } }
            )) {
                if let buffer = model.selectedBuffer {
                    MemberList(buffer: buffer)
                        .inspectorColumnWidth(min: 140, ideal: 180, max: 300)
                }
            }
        }
        .sheet(item: $model.sheet) { SheetContent(sheet: $0) }
        .toolbar {
            ToolbarItem(placement: .status) {
                ConnectionBadge(state: model.control)
            }
            ToolbarItem {
                Button {
                    showMembers.toggle()
                } label: {
                    Label("Members", systemImage: "person.2")
                }
                .help("Show or hide the member list")
                .disabled(model.selectedBuffer?.kind != .channel)
            }
            ToolbarItem {
                Menu {
                    Button("Log Out", role: .destructive) { model.logOut() }
                } label: {
                    Label("Account", systemImage: "person.crop.circle")
                }
            }
        }
    }
}

struct ConnectionBadge: View {
    let state: ConnectionState?

    var body: some View {
        let connected = state?.isConnected == true
        Label {
            Text(state?.summary ?? "Offline").lineLimit(1)
        } icon: {
            Circle()
                .fill(connected ? Color.green : Color.orange)
                .frame(width: 8, height: 8)
        }
        .help(state?.summary ?? "Offline")
    }
}

struct SidebarView: View {
    @Environment(AppModel.self) private var model
    @State private var collapsed: Set<String> = []
    @State private var addingNetwork = false

    var body: some View {
        List(selection: Binding(get: { model.selection }, set: { model.select($0) })) {
            ForEach(model.networks) { network in
                NetworkGroup(network: network, isExpanded: expanded(network.id))
            }
        }
        .listStyle(.sidebar)
        .overlay {
            if model.networks.isEmpty {
                ContentUnavailableView(
                    "No Networks", systemImage: "network",
                    description: Text("Add one to start chatting."))
            }
        }
        .safeAreaInset(edge: .bottom) {
            Button {
                addingNetwork = true
            } label: {
                Label("Add Network", systemImage: "plus")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.borderless)
            .padding(10)
        }
        .sheet(isPresented: $addingNetwork) {
            NavigationStack { NetworkFormView() }
        }
    }

    private func expanded(_ id: String) -> Binding<Bool> {
        Binding(
            get: { !collapsed.contains(id) },
            set: { if $0 { collapsed.remove(id) } else { collapsed.insert(id) } })
    }
}

/// A network: its row opens the server buffer; channels and private
/// conversations are nested under it.
struct NetworkGroup: View {
    @Environment(AppModel.self) private var model
    let network: NetworkState
    @Binding var isExpanded: Bool
    @State private var confirmingRemoval = false
    @State private var editing = false

    var body: some View {
        let server = network.buffers.first { model.buffers[$0]?.kind == .server }
        // Channels, then private conversations, each alphabetical.
        let children = network.buffers
            .filter { $0 != server }
            .sorted { a, b in
                let (ka, kb) = (model.buffers[a]?.kind, model.buffers[b]?.kind)
                if ka != kb { return ka == .channel }
                return a.key < b.key
            }
        DisclosureGroup(isExpanded: $isExpanded) {
            ForEach(children, id: \.self) { id in
                if let buffer = model.buffers[id] {
                    BufferRow(buffer: buffer)
                        .tag(id)
                        .contextMenu {
                            Button("Mark as Read") { model.markRead(id) }
                                .disabled(buffer.unread == 0)
                            Divider()
                            Button(buffer.kind == .channel ? "Leave Channel" : "Close Conversation") {
                                model.close(id)
                            }
                        }
                }
            }
        } label: {
            let unread = isExpanded
                ? server.flatMap { model.buffers[$0]?.unread } ?? 0
                : network.buffers.reduce(0) { $0 + (model.buffers[$1]?.unread ?? 0) }
            let row = NetworkRow(network: network, unread: unread)
                .contextMenu {
                    let connected = network.connection.isConnected
                    Button("Join Channel…") { model.sheet = .joinChannel(network: network.id) }
                        .disabled(!connected)
                    Button("Browse Channels…") { model.sheet = .browseChannels(network: network.id) }
                        .disabled(!connected)
                    Button("New Conversation…") { model.sheet = .newConversation(network: network.id) }
                        .disabled(!connected)
                    Divider()
                    Button("Edit Network…") { editing = true }
                    Divider()
                    Button("Remove Network…", role: .destructive) { confirmingRemoval = true }
                }
            if let server { row.tag(server) } else { row }
        }
        .sheet(isPresented: $editing) {
            NavigationStack { NetworkFormView(editing: network.network) }
        }
        .confirmationDialog(
            "Remove \(network.network.name)?", isPresented: $confirmingRemoval
        ) {
            Button("Remove Network", role: .destructive) { model.removeNetwork(network.id) }
        } message: {
            Text("soju disconnects from it and deletes its channels and history.")
        }
    }
}

struct NetworkRow: View {
    let network: NetworkState
    let unread: Int

    var body: some View {
        HStack {
            Image(systemName: "network")
                .foregroundStyle(network.connection.isConnected ? Color.secondary : Color.orange)
            Text(network.network.name)
                .fontWeight(.semibold)
                .lineLimit(1)
            Spacer()
            if !network.connection.isConnected {
                Image(systemName: "exclamationmark.circle")
                    .foregroundStyle(.orange)
                    .help(network.connection.summary)
            }
            UnreadBadge(count: unread)
        }
        .help(network.connection.summary)
    }
}

struct BufferRow: View {
    let buffer: BufferModel

    var body: some View {
        let unread = buffer.unread
        HStack {
            Image(systemName: buffer.kind == .channel ? "number" : "person")
                .foregroundStyle(.secondary)
            Text(buffer.name)
                .fontWeight(unread > 0 ? .semibold : .regular)
                .lineLimit(1)
            Spacer()
            UnreadBadge(count: unread)
        }
    }
}

struct UnreadBadge: View {
    let count: Int

    var body: some View {
        if count > 0 {
            Text("\(count)")
                .font(.caption.monospacedDigit())
                .padding(.horizontal, 6)
                .padding(.vertical, 1)
                .background(Capsule().fill(.tint))
                .foregroundStyle(.white)
        }
    }
}
