import AppKit
import BackroomCore
import SwiftUI

/// Sidebar, conversation and member list side by side under one full-width
/// title bar. Not `NavigationSplitView` (macOS 26 floats its sidebar around
/// the window buttons) nor `HSplitView` (it re-measures the side panels
/// whenever their content changes): the side panels keep the width the user
/// dragged them to.
struct MainView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.theme) private var theme
    @AppStorage("showMembers") private var showMembers = true
    @AppStorage("showSidebar") private var showSidebar = true
    @AppStorage("sidebarWidth") private var sidebarWidth = 220.0
    @AppStorage("membersWidth") private var membersWidth = 180.0

    var body: some View {
        @Bindable var model = model
        // Only channels have members; elsewhere the panel stays shut
        // without forgetting that it's wanted.
        let membersShown = showMembers && model.selectedBuffer?.kind == .channel
        HStack(spacing: 0) {
            if showSidebar {
                SidebarView()
                    .frame(width: sidebarWidth)
                SplitDivider(width: $sidebarWidth, range: 150...420, edge: .leading)
            }
            PaneTreeView(node: model.layout.root)
                .frame(minWidth: 320, maxWidth: .infinity, maxHeight: .infinity)
            if membersShown, let buffer = model.selectedBuffer {
                SplitDivider(width: $membersWidth, range: 120...360, edge: .trailing)
                MemberList(buffer: buffer)
                    .frame(width: membersWidth)
            }
        }
        .sheet(item: $model.sheet) { SheetContent(sheet: $0) }
        .navigationTitle(windowTitle)
        .toolbarBackground(theme.background == nil ? AnyShapeStyle(.bar) : AnyShapeStyle(theme.surfaceColor), for: .windowToolbar)
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button {
                    showSidebar.toggle()
                } label: {
                    Label("Sidebar", systemImage: "sidebar.left")
                }
                .help("Show or hide the sidebar")
                .keyboardShortcut("s", modifiers: [.command, .control])
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

extension MainView {
    /// The focused pane's conversation.
    private var windowTitle: String {
        guard let buffer = model.selectedBuffer else { return "Backroom" }
        return buffer.kind == .server ? model.networkName(buffer.id.network) : buffer.name
    }
}

/// The control connection's state, at the foot of the sidebar.
struct ConnectionStatus: View {
    @Environment(\.theme) private var theme
    let state: ConnectionState?

    var body: some View {
        let connected = state?.isConnected == true
        HStack(spacing: 6) {
            Circle()
                .fill(connected ? Color.green : Color.orange)
                .frame(width: 7, height: 7)
            Text(state?.summary ?? "Offline")
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .font(.caption)
        .foregroundStyle(theme.mutedColor)
        .help(state?.summary ?? "Offline")
    }
}

struct SidebarView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.theme) private var theme
    @State private var collapsed: Set<String> = []
    @State private var addingNetwork = false

    var body: some View {
        List(selection: Binding(get: { model.selection }, set: { model.select($0) })) {
            ForEach(model.networks) { network in
                NetworkGroup(network: network, isExpanded: expanded(network.id))
            }
        }
        .listStyle(.sidebar)
        // Themes with their own colours colour the sidebar too, a shade
        // darker than the chat; the default theme keeps the native look.
        .scrollContentBackground(theme.background == nil ? .automatic : .hidden)
        .background {
            if theme.background != nil {
                theme.backgroundColor.overlay(Color.black.opacity(theme.isDark ? 0.22 : 0.05))
            }
        }
        .foregroundStyle(theme.textColor)
        .overlay {
            if model.networks.isEmpty {
                ContentUnavailableView(
                    "No Networks", systemImage: "network",
                    description: Text("Add one to start chatting."))
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 6) {
            Button {
                addingNetwork = true
            } label: {
                Label("Add Network", systemImage: "plus")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.borderless)
            .foregroundStyle(theme.textColor)
            .padding(.horizontal, 10)
            .padding(.top, 6)
            ConnectionStatus(state: model.control)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
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
                            Button("Open in Split Right") { model.split(.horizontal, showing: id) }
                            Button("Open in Split Down") { model.split(.vertical, showing: id) }
                            Divider()
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
            let counted = isExpanded ? (server.map { [$0] } ?? []) : network.buffers
            let unread = counted.reduce(0) { $0 + (model.buffers[$1]?.unread ?? 0) }
            let mentions = counted.reduce(0) { $0 + (model.buffers[$1]?.unreadMentions ?? 0) }
            let row = NetworkRow(network: network, unread: unread, mentions: mentions)
                .contextMenu {
                    let connected = network.connection.isConnected
                    Button("Join Channel…") { model.sheet = .joinChannel(network: network.id) }
                        .disabled(!connected)
                    Button("Browse Channels…") { model.sheet = .browseChannels(network: network.id) }
                        .disabled(!connected)
                    Button("New Conversation…") { model.sheet = .newConversation(network: network.id) }
                        .disabled(!connected)
                    Divider()
                    Button(network.away ? "Set Back" : "Set Away") {
                        model.setAway(network: network.id, away: !network.away)
                    }
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
    @Environment(\.theme) private var theme
    let network: NetworkState
    let unread: Int
    let mentions: Int

    var body: some View {
        HStack {
            Image(systemName: "network")
                .foregroundStyle(network.connection.isConnected ? theme.mutedColor : Color.orange)
            Text(network.network.name)
                .fontWeight(.semibold)
                .lineLimit(1)
            Spacer()
            if network.away {
                Image(systemName: "moon.fill")
                    .foregroundStyle(theme.mutedColor)
                    .help("You're marked away")
            }
            if !network.connection.isConnected {
                Image(systemName: "exclamationmark.circle")
                    .foregroundStyle(.orange)
                    .help(network.connection.summary)
            }
            UnreadBadge(count: unread, mentions: mentions)
        }
        .help(network.connection.summary)
    }
}

struct BufferRow: View {
    @Environment(\.theme) private var theme
    let buffer: BufferModel

    var body: some View {
        let unread = buffer.unread
        HStack {
            Image(systemName: buffer.kind == .channel ? "number" : "person")
                .foregroundStyle(theme.mutedColor)
            Text(buffer.name)
                .fontWeight(unread > 0 ? .semibold : .regular)
                .lineLimit(1)
                .help(buffer.name)
            Spacer()
            UnreadBadge(count: unread, mentions: buffer.unreadMentions)
        }
    }
}

/// Unread count; red when something mentions you or is a private message.
struct UnreadBadge: View {
    let count: Int
    var mentions = 0

    var body: some View {
        if count > 0 {
            Text("\(count)")
                .font(.caption.monospacedDigit())
                .padding(.horizontal, 6)
                .padding(.vertical, 1)
                .background(Capsule().fill(mentions > 0 ? AnyShapeStyle(Color.red) : AnyShapeStyle(.tint)))
                .foregroundStyle(.white)
                .help(mentions > 0 ? "\(count) unread, \(mentions) for you" : "\(count) unread")
        }
    }
}

/// A draggable line that sets the width of the panel on its `edge` side.
struct SplitDivider: View {
    @Binding var width: Double
    let range: ClosedRange<Double>
    /// Which side the panel is on: dragging towards it narrows it.
    let edge: HorizontalEdge
    @State private var start: Double?

    var body: some View {
        Rectangle()
            .fill(Color(nsColor: .separatorColor))
            .frame(width: 1)
            // A wider grab area than the visible line.
            .overlay {
                Color.clear
                    .frame(width: 9)
                    .contentShape(Rectangle())
                    .onHover { inside in
                        if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
                    }
                    .gesture(
                        DragGesture(minimumDistance: 1, coordinateSpace: .global)
                            .onChanged { drag in
                                let base = start ?? width
                                start = base
                                let delta = edge == .leading ? drag.translation.width : -drag.translation.width
                                width = min(max(base + delta, range.lowerBound), range.upperBound)
                            }
                            .onEnded { _ in start = nil }
                    )
            }
            .zIndex(1)
    }
}
