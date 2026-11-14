import BackroomCore
import Foundation
import Observation

/// Opens the login page and returns the URL it redirects to.
protocol WebAuthenticator {
    @MainActor func authenticate(url: URL, callbackScheme: String) async throws -> URL
    /// Abandons a login in progress; `authenticate` throws `CancellationError`.
    @MainActor func cancel()
}

/// All app state. Owns the Rust client and applies its events on the main
/// actor; views read from here and call its methods.
@MainActor @Observable
final class AppModel {
    enum Phase: Equatable {
        case loggedOut
        case loggingIn
        case online
        /// Couldn't start with the saved login (e.g. Keycloak unreachable).
        case failed(String)
    }

    private(set) var phase: Phase = .loggedOut
    private(set) var settings: ServerSettings
    /// The control connection's state.
    private(set) var control: ConnectionState?
    private(set) var networks: [NetworkState] = []
    /// Changes only when buffers come and go; each buffer is observed on
    /// its own.
    private(set) var buffers: [BufferID: BufferModel] = [:]
    /// The chat area's panes and which one has the focus. Kept across
    /// reconnects and launches; panes show their buffer once it exists.
    /// Tabs, each with its own pane layout; saved on every change.
    private(set) var tabs: [WorkspaceTab] {
        didSet { if tabs != oldValue { saveLayout() } }
    }
    private(set) var currentTab: WorkspaceTab.ID {
        didSet { if currentTab != oldValue { saveLayout() } }
    }
    /// The current tab's panes.
    private(set) var layout: PaneLayout {
        get { tabs.first { $0.id == currentTab }?.layout ?? tabs[0].layout }
        set {
            let i = tabs.firstIndex { $0.id == currentTab } ?? 0
            tabs[i].layout = newValue
        }
    }
    /// The focused pane's buffer: what the member list, window title and
    /// sidebar highlight follow.
    var selection: BufferID? { layout.focusedPane?.buffer }
    /// The latest error worth showing; the view clears it.
    var notice: String?
    /// A sheet to show over the main window (from menus and the sidebar).
    var sheet: Sheet?
    /// Channel lists by network, from "Browse Channels".
    private(set) var channelLists: [String: ChannelList] = [:]

    enum Sheet: Identifiable, Equatable {
        case joinChannel(network: String)
        case browseChannels(network: String)
        case newConversation(network: String)
        case whois(network: String, info: WhoisInfo)
        case search(network: String)
        case channelSettings(network: String, channel: String)

        var id: String {
            switch self {
            case .search(let n): "search-\(n)"
            case .channelSettings(let n, let c): "channel-\(n)-\(c)"
            case .joinChannel(let n): "join-\(n)"
            case .browseChannels(let n): "browse-\(n)"
            case .newConversation(let n): "query-\(n)"
            case .whois(let n, let info): "whois-\(n)-\(info.nick)"
            }
        }
    }

    /// The latest search and its results.
    struct SearchState {
        var network: String
        var text: String
        /// Buffers whose results are still to come.
        var pending: Set<String>
        var hits: [SearchHit] = []
        var errors: [String] = []
    }
    private(set) var search: SearchState?

    /// What the channel settings sheet shows.
    struct ChannelSettings: Equatable {
        var network: String
        var channel: String
        var modes: String?
        var params: [String] = []
        var bans: [BanEntry]?
    }
    private(set) var channelSettings: ChannelSettings?

    /// Called for each live message that should notify: mentions and
    /// private messages (or any channel message, if wanted) that arrive
    /// while their buffer isn't in view.
    @ObservationIgnored var notify: ((BufferModel, ChatMessage) -> Void)?

    struct ChannelList: Equatable {
        var channels: [ChannelListing] = []
        var loading = true
    }

    static let historyPage: UInt32 = 100

    typealias ClientFactory = @MainActor (ServerSettings) throws -> any BackroomClientProtocol

    /// Most recently viewed first; where to go when the open buffer goes.
    @ObservationIgnored private var recent: [BufferID] = []
    /// A channel the user just /joined; selected once it exists.
    @ObservationIgnored private var pendingSelection: BufferID?
    @ObservationIgnored private let defaults: UserDefaults
    /// Highlight words and the ignore list come from here.
    @ObservationIgnored let preferences: Preferences
    /// Before panes: the one buffer that was open.
    private static let lastBufferKey = "lastBuffer"
    private static let layoutKey = "paneLayout"
    private static let tabsKey = "tabs"

    @ObservationIgnored private var client: (any BackroomClientProtocol)?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private let makeClient: ClientFactory
    @ObservationIgnored private let authenticator: any WebAuthenticator
    /// Whether the app is frontmost; buffers are only marked read then.
    @ObservationIgnored var isActive = true {
        didSet {
            guard isActive else { return }
            for id in layout.visibleBuffers {
                // Coming back to the app: mark where the unseen messages start.
                if let buffer = buffers[id], buffer.unread > 0 { buffer.unreadLine = buffer.readMarker }
                markRead(id)
            }
        }
    }

    init(settings: ServerSettings, authenticator: any WebAuthenticator,
         defaults: UserDefaults = .standard, preferences: Preferences? = nil,
         makeClient: @escaping ClientFactory) {
        self.settings = settings
        self.defaults = defaults
        self.preferences = preferences ?? Preferences(defaults: defaults)
        let saved: (tabs: [WorkspaceTab], current: UUID)? = defaults.data(forKey: Self.tabsKey)
            .flatMap { try? JSONDecoder().decode(SavedTabs.self, from: $0) }
            .flatMap { $0.tabs.isEmpty ? nil : ($0.tabs, $0.current) }
        if let saved {
            tabs = saved.tabs
            currentTab = saved.tabs.contains { $0.id == saved.current } ? saved.current : saved.tabs[0].id
        } else {
            // Earlier versions: one layout, or before that one buffer.
            let layout = defaults.data(forKey: Self.layoutKey)
                .flatMap { try? JSONDecoder().decode(PaneLayout.self, from: $0) }
                ?? defaults.stringArray(forKey: Self.lastBufferKey).flatMap { last in
                    last.count == 2 ? PaneLayout(buffer: BufferID(network: last[0], name: last[1])) : nil
                }
                ?? PaneLayout()
            let tab = WorkspaceTab(layout: layout)
            tabs = [tab]
            currentTab = tab.id
        }
        self.authenticator = authenticator
        self.makeClient = makeClient
    }

    // MARK: Lifecycle

    /// Creates the client for the current settings and, with `connect`,
    /// connects if a login is saved.
    func start(connect autoConnect: Bool = true) async {
        stopSession()
        guard settings.isComplete else {
            phase = .loggedOut
            return
        }
        do {
            let client = try makeClient(settings)
            self.client = client
            if autoConnect, try client.hasLogin() {
                await connect()
            } else {
                phase = .loggedOut
            }
        } catch {
            phase = .failed(Self.describe(error))
        }
    }

    /// Switches to `new` without connecting, ready to log in.
    private func use(_ new: ServerSettings) async {
        guard new != settings || client == nil else { return }
        settings = new
        new.save(to: defaults)
        await start(connect: false)
    }

    /// The login screen: saves `new` and logs in with a password.
    func logIn(_ new: ServerSettings, password: String) async {
        guard phase != .loggingIn, new.login == .password, new.isComplete, !password.isEmpty else { return }
        await use(new)
        await logIn(password: password)
    }

    /// The login screen: saves `new` and logs in with single sign-on.
    func logIn(_ new: ServerSettings) async {
        guard phase != .loggingIn, new.login == .singleSignOn, new.isComplete else { return }
        await use(new)
        await logIn()
    }

    /// Saves the password of a password account and connects; soju checks
    /// it, and a wrong one comes back as `authFailed`.
    func logIn(password: String) async {
        guard let client, phase != .loggingIn, !password.isEmpty else { return }
        do {
            try client.loginWithPassword(password: password)
        } catch {
            if case .failed = phase {} else { phase = .loggedOut }
            notice = "Login failed: \(Self.describe(error))"
            return
        }
        notice = nil
        await connect()
    }

    /// Single sign-on: opens the login page in the browser and connects
    /// once it redirects back.
    func logIn() async {
        guard let client, phase != .loggingIn else { return }
        phase = .loggingIn
        do {
            let address = try await client.beginLogin()
            guard let url = URL(string: address) else { throw LoginError.badURL }
            let callback = try await authenticator.authenticate(
                url: url, callbackScheme: ServerSettings.callbackScheme)
            try await client.completeLogin(callbackUrl: callback.absoluteString)
            await connect()
        } catch is CancellationError {
            phase = .loggedOut
        } catch {
            phase = .loggedOut
            notice = "Login failed: \(Self.describe(error))"
        }
    }

    func cancelLogin() {
        authenticator.cancel()
    }

    func logOut() {
        do { try client?.logout() } catch { notice = Self.describe(error) }
        eventTask = nil
        resetChat()
        phase = .loggedOut
    }

    /// Saves new settings and starts over with them.
    func apply(settings new: ServerSettings) async {
        settings = new
        new.save(to: defaults)
        await start()
    }

    private func connect() async {
        guard let client else { return }
        resetChat()
        control = .connecting
        phase = .online
        do {
            try await client.connect()
        } catch BackroomError.NotLoggedIn {
            phase = .loggedOut
            return
        } catch {
            phase = .failed(Self.describe(error))
            return
        }
        eventTask = Task { [weak self, client] in
            while let event = await client.nextEvent() {
                self?.apply(event)
            }
        }
    }

    private func stopSession() {
        client?.disconnect()
        client = nil
        eventTask = nil
        resetChat()
    }

    private func resetChat() {
        control = nil
        networks = []
        buffers = [:]
        recent = []
        sheet = nil
        channelLists = [:]
    }

    // MARK: Events

    func apply(_ event: SessionEvent) {
        switch event {
        case .connection(nil, let state):
            control = state
            if case .authFailed(let reason) = state {
                client?.disconnect()
                resetChat()
                phase = .loggedOut
                notice = Self.loginRejected(reason, login: settings.login)
            }
        case .connection(let id?, let state):
            if let i = networkIndex(id) { networks[i].connection = state }
        case .networkUpdated(let network):
            if let i = networkIndex(network.id) {
                networks[i].network = network
            } else {
                networks.append(NetworkState(network: network))
            }
        case .networkRemoved(let id):
            networks.removeAll { $0.id == id }
            buffers = buffers.filter { $0.key.network != id }
            recent.removeAll { $0.network == id }
            for t in tabs.indices {
                for pane in tabs[t].layout.panes where pane.buffer?.network == id {
                    tabs[t].layout.show(nil, in: pane.id)
                }
            }
        case .bufferAdded(let network, let name, let kind):
            ensureBuffer(network: network, name: name, kind: kind)
        case .bufferRemoved(let network, let name):
            let id = BufferID(network: network, name: name)
            buffers[id] = nil
            recent.removeAll { $0 == id }
            if let i = networkIndex(network) { networks[i].buffers.removeAll { $0 == id } }
            // Panes showing it go back to the previous conversation, else
            // the network's server buffer.
            let fallback = recent.first { buffers[$0] != nil }
                ?? networkIndex(network).flatMap { i in
                    networks[i].buffers.first { buffers[$0]?.kind == .server }
                }
            for t in tabs.indices {
                for pane in tabs[t].layout.panes where pane.buffer == id {
                    tabs[t].layout.show(fallback, in: pane.id)
                }
            }
            if let fallback, layout.visibleBuffers.contains(fallback) { activate(fallback) }
        case .topic(let network, let channel, let topic):
            buffers[BufferID(network: network, name: channel)]?.topic = topic
        case .members(let network, let channel, let list):
            buffers[BufferID(network: network, name: channel)]?.members = list
        case .message(let network, let name, let message):
            let id = ensureBuffer(network: network, name: name, kind: nil)
            guard let buffer = buffers[id] else { return }
            buffer.merge([message])
            // A message ends its sender's typing.
            buffer.typing[message.sender.lowercased()] = nil
            let inView = isActive && layout.visibleBuffers.contains(id)
            if layout.visibleBuffers.contains(id) { markRead(id) }
            if !inView, shouldNotify(buffer, message) { notify?(buffer, message) }
        // History and read markers only update buffers that still exist:
        // they can arrive after we left a channel, and mustn't bring it back.
        case .history(let network, let name, let messages):
            let id = BufferID(network: network, name: name)
            guard let buffer = buffers[id] else { return }
            buffer.merge(messages)
            buffer.historyLoading = false
            if messages.count < Self.historyPage { buffer.historyExhausted = true }
            if layout.visibleBuffers.contains(id) { markRead(id) }
            loadTowardsTarget(id)
        case .readMarker(let network, let name, let time):
            // Markers only move forward: soju's reply to the initial query
            // can arrive after this device already marked the buffer read.
            guard let buffer = buffers[BufferID(network: network, name: name)] else { return }
            if let time, buffer.readMarker.map({ time > $0 }) ?? true {
                buffer.readMarker = time
            }
        case .channelList(let network, let channels, let done):
            // Chunks only extend a list someone asked for.
            guard channelLists[network] != nil else { return }
            channelLists[network]?.channels.append(contentsOf: channels)
            if done { channelLists[network]?.loading = false }
        case .searchResults(let network, let target, let text, let results, let error):
            guard var state = search, state.network == network, state.text == text,
                state.pending.remove(target.lowercased()) != nil
            else { return }
            state.hits.append(contentsOf: results)
            state.hits.sort { $0.message.time > $1.message.time }
            if let error { state.errors.append("\(target): \(error)") }
            search = state
        case .typing(let network, let name, let nick, let state):
            buffers[BufferID(network: network, name: name)]?.typing[nick.lowercased()] = (nick, state, .now)
        case .presence(let network, let nick, let online):
            buffers[BufferID(network: network, name: nick)]?.online = online
        case .channelModes(let network, let channel, let modes, let params):
            if channelSettings?.network == network, channelSettings?.channel.lowercased() == channel.lowercased() {
                channelSettings?.modes = modes
                channelSettings?.params = params
            }
        case .banList(let network, let channel, let bans):
            if channelSettings?.network == network, channelSettings?.channel.lowercased() == channel.lowercased() {
                channelSettings?.bans = bans
            }
        case .ownNick(let network, let nick):
            if let i = networkIndex(network) {
                networks[i].nick = nick
                refreshMentions(network: network)
            }
        case .away(let network, let away):
            if let i = networkIndex(network) { networks[i].away = away }
        case .whois(let network, let info):
            sheet = .whois(network: network, info: info)
        case .error(_, let message):
            notice = message
        case .stopped:
            control = nil
        }
    }

    @discardableResult
    private func ensureBuffer(network: String, name: String, kind: BufferKind?) -> BufferID {
        let id = BufferID(network: network, name: name)
        if buffers[id] == nil {
            let kind = kind ?? Self.guessKind(name)
            let buffer = BufferModel(id: id, name: name, kind: kind)
            buffer.mentions = mentionMatcher(network: network)
            buffers[id] = buffer
            if let i = networkIndex(network) {
                // The server buffer goes first.
                if kind == .server {
                    networks[i].buffers.insert(id, at: 0)
                } else {
                    networks[i].buffers.append(id)
                }
            }
            if id == pendingSelection {
                pendingSelection = nil
                select(id)
            } else if layout.visibleBuffers.contains(id) {
                // A pane from last time shows it again.
                activate(id)
            }
        } else if let kind {
            buffers[id]?.kind = kind
        }
        return id
    }

    private static func guessKind(_ name: String) -> BufferKind {
        if name == "*" { return .server }
        return name.first.map { "#&+!".contains($0) } == true ? .channel : .query
    }

    private func networkIndex(_ id: String) -> Int? {
        networks.firstIndex { $0.id == id }
    }

    // MARK: Actions

    /// Shows a buffer in the focused pane.
    func select(_ id: BufferID?) {
        layout.show(id, in: layout.focused)
        if let id { activate(id) }
    }

    /// A buffer came into view: load its history and mark it read.
    private func activate(_ id: BufferID) {
        guard let buffer = buffers[id], let client else { return }
        recent.removeAll { $0 == id }
        recent.insert(id, at: 0)
        // Before marking read: the line shows where this visit's news starts.
        buffer.unreadLine = buffer.readMarker
        if !buffer.historyRequested {
            buffers[id]?.historyRequested = true
            buffers[id]?.historyLoading = true
            run { try client.loadHistory(network: id.network, target: buffer.name, before: nil, limit: Self.historyPage) }
        }
        markRead(id)
    }

    private func saveLayout() {
        let saved = SavedTabs(tabs: tabs, current: currentTab)
        if let data = try? JSONEncoder().encode(saved) { defaults.set(data, forKey: Self.tabsKey) }
    }

    private struct SavedTabs: Codable {
        var tabs: [WorkspaceTab]
        var current: UUID
    }

    // MARK: Tabs

    /// A new tab showing the current conversation, after the current tab.
    func newTab() {
        let tab = WorkspaceTab(layout: PaneLayout(buffer: selection))
        let i = tabs.firstIndex { $0.id == currentTab } ?? tabs.count - 1
        tabs.insert(tab, at: i + 1)
        showTab(tab.id)
    }

    func showTab(_ id: WorkspaceTab.ID) {
        guard tabs.contains(where: { $0.id == id }) else { return }
        currentTab = id
        // Its panes come into view: load and mark read.
        for buffer in layout.visibleBuffers { activate(buffer) }
    }

    /// The tab `offset` places away from the current one, wrapping around.
    func cycleTab(by offset: Int) {
        guard let i = tabs.firstIndex(where: { $0.id == currentTab }) else { return }
        showTab(tabs[(i + offset + tabs.count) % tabs.count].id)
    }

    /// Tab number `n` (from 1), as ⌘1–9 do; ⌘9 is the last tab.
    func showTab(number n: Int) {
        guard !tabs.isEmpty else { return }
        showTab(tabs[n == 9 ? tabs.count - 1 : min(n - 1, tabs.count - 1)].id)
    }

    /// Closes a tab (never the last one); its neighbour is shown.
    func closeTab(_ id: WorkspaceTab.ID) {
        guard tabs.count > 1, let i = tabs.firstIndex(where: { $0.id == id }) else { return }
        let wasCurrent = id == currentTab
        tabs.remove(at: i)
        if wasCurrent { showTab(tabs[min(i, tabs.count - 1)].id) }
    }

    func renameTab(_ id: WorkspaceTab.ID, to name: String) {
        guard let i = tabs.firstIndex(where: { $0.id == id }) else { return }
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        tabs[i].name = trimmed.isEmpty ? nil : trimmed
    }

    /// Moves a tab to where `target` is.
    func moveTab(_ id: WorkspaceTab.ID, to target: WorkspaceTab.ID) {
        guard let from = tabs.firstIndex(where: { $0.id == id }),
            let to = tabs.firstIndex(where: { $0.id == target }), from != to
        else { return }
        let tab = tabs.remove(at: from)
        tabs.insert(tab, at: to)
    }

    /// The tab's name, or its conversations' names.
    func tabTitle(_ tab: WorkspaceTab) -> String {
        if let name = tab.name { return name }
        let names = tab.layout.panes.compactMap(\.buffer).compactMap { id -> String? in
            guard let buffer = buffers[id] else { return id.key }
            return buffer.kind == .server ? networkName(id.network) : buffer.name
        }
        var seen = Set<String>()
        let unique = names.filter { seen.insert($0.lowercased()).inserted }
        return unique.isEmpty ? "Empty" : unique.joined(separator: ", ")
    }

    /// Unread and mention counts for a tab's conversations.
    func tabUnread(_ tab: WorkspaceTab) -> (all: Int, mentions: Int) {
        tab.layout.visibleBuffers.reduce((0, 0)) { sum, id in
            guard let buffer = buffers[id] else { return sum }
            return (sum.0 + buffer.unread, sum.1 + buffer.unreadMentions)
        }
    }

    /// ⌘W: the focused pane; the tab when it's down to one pane.
    var canClosePane: Bool { layout.panes.count > 1 || tabs.count > 1 }

    // MARK: Panes

    func focus(_ pane: Pane.ID) {
        guard layout.focused != pane, layout.pane(pane) != nil else { return }
        layout.focused = pane
    }

    /// Splits the focused pane; the new pane shows `buffer` (by default the
    /// same buffer) and gets the focus.
    func split(_ axis: SplitAxis, showing buffer: BufferID? = nil) {
        let shown = buffer ?? selection
        layout.split(layout.focused, axis: axis, buffer: shown)
        if let shown { activate(shown) }
    }

    /// Closes a pane (the focused one by default); never the last.
    func closePane(_ pane: Pane.ID? = nil) {
        if layout.panes.count == 1 {
            closeTab(currentTab)
        } else {
            layout.close(pane ?? layout.focused)
        }
    }

    func closeOtherPanes() {
        layout.closeOthers()
    }

    func moveFocus(_ direction: PaneDirection) {
        if let next = layout.neighbour(direction) { layout.focused = next }
    }

    func resizeSplit(_ split: UUID, fractions: [Double]) {
        layout.resize(split: split, fractions: fractions)
    }

    var selectedBuffer: BufferModel? { selection.flatMap { buffers[$0] } }

    /// Opens (or switches to) a private conversation with `nick`.
    func openQuery(network: String, nick: String) {
        let id = ensureBuffer(network: network, name: nick, kind: .query)
        select(id)
    }

    // MARK: Notifications

    private func shouldNotify(_ buffer: BufferModel, _ message: ChatMessage) -> Bool {
        guard preferences.notifications, !message.own, !buffer.mentions.isIgnored(message.sender)
        else { return false }
        if buffer.isMention(message) { return true }
        switch buffer.kind {
        case .query: return message.kind.isChat
        case .channel: return preferences.notifyAllMessages && message.kind.isChat
        case .server: return false
        }
    }

    /// Unread mentions and private messages everywhere, for the Dock icon.
    var totalMentions: Int { buffers.values.reduce(0) { $0 + $1.unreadMentions } }

    /// Brings a buffer into view (from a notification): in a pane of the
    /// current tab if one shows it, else in the focused pane.
    func reveal(_ id: BufferID) {
        guard buffers[id] != nil else { return }
        if let pane = layout.panes.first(where: { $0.buffer == id }) {
            layout.focused = pane.id
            activate(id)
        } else {
            select(id)
        }
    }

    /// Shows a message from search: selects its buffer and scrolls to it,
    /// loading older history until it's there (up to 20 pages).
    func reveal(network: String, buffer name: String, message: ChatMessage) {
        let id = ensureBuffer(network: network, name: name, kind: nil)
        reveal(id)
        guard let buffer = buffers[id] else { return }
        buffer.scrollTarget = message.stableID
        pagesToTarget = 20
        loadTowardsTarget(id)
    }

    @ObservationIgnored private var pagesToTarget = 0

    private func loadTowardsTarget(_ id: BufferID) {
        guard let buffer = buffers[id], let target = buffer.scrollTarget else { return }
        if buffer.messages.contains(where: { $0.stableID == target }) || buffer.historyExhausted { return }
        guard pagesToTarget > 0 else {
            notice = "That message is too far back to show here."
            buffer.scrollTarget = nil
            return
        }
        pagesToTarget -= 1
        if buffer.historyRequested { loadOlder(id) }
    }

    // MARK: Search and channel settings

    /// Searches one buffer, or every conversation on the network (nil).
    func startSearch(network: String, in target: String?, text: String) {
        guard let client else { return }
        let text = text.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return }
        let targets = target.map { [$0] } ?? networkIndex(network).map { i in
            networks[i].buffers.compactMap { buffers[$0] }.filter { $0.kind != .server }.map(\.name)
        } ?? []
        search = SearchState(network: network, text: text, pending: Set(targets.map { $0.lowercased() }))
        for t in targets {
            run { try client.search(network: network, target: t, text: text) }
        }
    }

    func openChannelSettings(network: String, channel: String) {
        guard let client else { return }
        channelSettings = ChannelSettings(network: network, channel: channel)
        sheet = .channelSettings(network: network, channel: channel)
        run { try client.loadChannelSettings(network: network, channel: channel) }
    }

    /// Typing state for a buffer's conversation (not the server buffer).
    func setTyping(_ id: BufferID, _ state: TypingState) {
        guard let client, let buffer = buffers[id], buffer.kind != .server else { return }
        try? client.setTyping(network: id.network, target: buffer.name, state: state)
    }

    // MARK: Mentions and ignores

    private func mentionMatcher(network: String) -> MentionMatcher {
        MentionMatcher(
            nick: networkIndex(network).flatMap { networks[$0].nick },
            words: preferences.highlightWords, ignored: preferences.ignored)
    }

    /// Rebuilds mention matching after our nick, the highlight words or the
    /// ignore list changed (`network` nil: everywhere).
    func refreshMentions(network: String? = nil) {
        for (id, buffer) in buffers where network == nil || id.network == network {
            let matcher = mentionMatcher(network: id.network)
            if buffer.mentions != matcher { buffer.mentions = matcher }
        }
    }

    // MARK: IRC actions

    /// Sends a raw line on a network.
    @discardableResult
    func sendRaw(network: String, _ line: String) -> Bool {
        guard let client else { return false }
        return run { try client.sendRaw(network: network, line: line) }
    }

    func setAway(network: String, away: Bool) {
        sendRaw(network: network, away ? "AWAY :Away" : "AWAY")
    }

    /// Our nick on a network, once known.
    func ownNick(network: String) -> String? {
        networkIndex(network).flatMap { networks[$0].nick }
    }

    /// Our membership prefix in a channel ("" if none, nil if unknown).
    func ownPrefix(in buffer: BufferModel) -> String? {
        ownNick(network: buffer.id.network).flatMap { buffer.prefixByNick[$0.lowercased()] }
    }

    /// The network menu commands act on: the selected buffer's, else the first.
    var currentNetwork: String? { selection?.network ?? networks.first?.id }

    /// Joins channels (comma-separated) and switches to the first: now if
    /// we're already in it, else once the server confirms.
    @discardableResult
    func join(network: String, channel: String, key: String? = nil) -> Bool {
        guard let client else { return false }
        let target = BufferID(network: network, name: String(channel.split(separator: ",").first ?? ""))
        if buffers[target] != nil {
            select(target)
        } else {
            pendingSelection = target
        }
        return run { try client.join(network: network, channel: channel, key: key) }
    }

    /// Asks for a network's channel list, replacing any earlier one.
    func listChannels(network: String) {
        guard let client else { return }
        channelLists[network] = ChannelList()
        if !run({ try client.listChannels(network: network) }) {
            channelLists[network]?.loading = false
        }
    }

    /// Asks soju to add a network; it appears once soju has it.
    func addNetwork(_ fields: NetworkFields) {
        guard let client else { return }
        run { try client.addNetwork(attributes: fields.attributes(keepEmpty: false)) }
    }

    /// Changes a network's settings; soju reconnects to it.
    func changeNetwork(_ id: String, _ fields: NetworkFields) {
        guard let client else { return }
        // Empty values clear attributes, e.g. a removed nick.
        run { try client.changeNetwork(network: id, attributes: fields.attributes(keepEmpty: true)) }
    }

    /// Deletes a network from soju, with its channels and history.
    func removeNetwork(_ id: String) {
        guard let client else { return }
        run { try client.removeNetwork(network: id) }
    }

    /// Leaves a channel or closes a private conversation.
    func close(_ id: BufferID) {
        guard let buffer = buffers[id], let client else { return }
        run { try client.closeBuffer(network: id.network, name: buffer.name) }
    }

    /// Asks for the page before the oldest loaded message.
    func loadOlder(_ id: BufferID) {
        guard let buffer = buffers[id], let client, !buffer.historyLoading,
            !buffer.historyExhausted, !buffer.cleared, let oldest = buffer.messages.first
        else { return }
        buffers[id]?.historyLoading = true
        run {
            try client.loadHistory(network: id.network, target: buffer.name, before: oldest.time, limit: Self.historyPage)
        }
    }

    /// Moves the read marker to the newest message, here and on soju.
    func markRead(_ id: BufferID) {
        guard isActive, let buffer = buffers[id], let client, let newest = buffer.messages.last?.time
        else { return }
        if let marker = buffer.readMarker, marker >= newest { return }
        buffers[id]?.readMarker = newest
        run { try client.markRead(network: id.network, target: buffer.name, time: newest) }
    }

    /// Sends a composer line to the selected buffer. Returns true if it was
    /// handled and the composer should be cleared.
    func send(_ input: String) -> Bool {
        guard let id = selection, let buffer = buffers[id], let client,
            let action = Composer.parse(input, buffer: buffer.name)
        else { return false }
        switch action {
        case .message(let text), .action(let text):
            guard buffer.kind != .server else {
                notice = "Messages can't go to the server buffer. Use /join #channel."
                return false
            }
            let line = if case .action = action { Composer.actionText(text) } else { text }
            return run { try client.sendMessage(network: id.network, target: buffer.name, text: line) }
        case .join(let channel, let key):
            return join(network: id.network, channel: channel, key: key)
        case .part(let channel, let reason):
            return run { try client.part(network: id.network, channel: channel, reason: reason) }
        case .query(let nick, let text):
            openQuery(network: id.network, nick: nick)
            guard let text else { return true }
            return run { try client.sendMessage(network: id.network, target: nick, text: text) }
        case .raw(let line):
            return run { try client.sendRaw(network: id.network, line: line) }
        case .cycle(let channel):
            pendingSelection = id
            return run {
                try client.part(network: id.network, channel: channel, reason: nil)
                try client.join(network: id.network, channel: channel, key: nil)
            }
        case .listChannels:
            sheet = .browseChannels(network: id.network)
            return true
        case .clear:
            buffer.clear()
            return true
        case .close:
            if buffer.kind == .server {
                notice = "The server buffer can't be closed."
                return false
            }
            close(id)
            return true
        case .ignore(let nick):
            preferences.ignore(nick)
            refreshMentions()
            notice = "Ignoring \(nick). /unignore \(nick) undoes it."
            return true
        case .unignore(let nick):
            preferences.unignore(nick)
            refreshMentions()
            return true
        case .invalid(let message):
            notice = message
            return false
        }
    }

    @discardableResult
    private func run(_ body: () throws -> Void) -> Bool {
        do {
            try body()
            return true
        } catch {
            notice = Self.describe(error)
            return false
        }
    }

    // MARK: Helpers

    private enum LoginError: Error, CustomStringConvertible {
        case badURL
        var description: String { "the server returned an invalid login URL" }
    }

    /// What to say when soju turns the login down.
    static func loginRejected(_ reason: String, login: ServerSettings.Login) -> String {
        // From the core when soju doesn't offer our SASL mechanism.
        if reason.contains("doesn't accept OAUTHBEARER") {
            return "This server doesn't support single sign-on. Check the server address, or log in with a username and password."
        }
        if reason.contains("doesn't accept PLAIN") {
            return "This server doesn't accept passwords; log in with single sign-on."
        }
        switch login {
        // soju's 904 text is generic ("Authentication failed").
        case .password: return "Wrong username or password."
        case .singleSignOn: return "Your login was rejected (\(reason)). Log in again."
        }
    }

    static func describe(_ error: Error) -> String {
        guard let error = error as? BackroomError else { return error.localizedDescription }
        switch error {
        case .NotLoggedIn(let message), .Auth(let message), .Config(let message),
            .Store(let message), .Internal(let message):
            return message
        case .NoLoginInProgress: return "no login in progress"
        case .NotConnected: return "not connected"
        }
    }
}
