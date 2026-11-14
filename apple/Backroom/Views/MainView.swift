import BackroomCore
import SwiftUI

struct MainView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 180, ideal: 220)
        } detail: {
            if let id = model.selection, model.buffers[id] != nil {
                BufferView(id: id)
                    .id(id)
            } else {
                ContentUnavailableView("No Conversation Selected", systemImage: "bubble.left")
            }
        }
        .toolbar {
            ToolbarItem(placement: .status) {
                ConnectionBadge(state: model.control)
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
                    description: Text("Networks you add in soju appear here."))
            }
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

    var body: some View {
        let server = network.buffers.first { model.buffers[$0]?.kind == .server }
        let children = network.buffers.filter { $0 != server }
        DisclosureGroup(isExpanded: $isExpanded) {
            ForEach(children, id: \.self) { id in
                if let buffer = model.buffers[id] {
                    BufferRow(buffer: buffer).tag(id)
                }
            }
        } label: {
            let unread = isExpanded
                ? server.flatMap { model.buffers[$0]?.unread } ?? 0
                : network.buffers.reduce(0) { $0 + (model.buffers[$1]?.unread ?? 0) }
            let row = NetworkRow(network: network, unread: unread)
            if let server { row.tag(server) } else { row }
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
    let buffer: BufferState

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
