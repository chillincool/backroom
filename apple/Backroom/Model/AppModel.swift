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
    private(set) var selection: BufferID?
    /// The latest error worth showing; the view clears it.
    var notice: String?

    static let historyPage: UInt32 = 100

    typealias ClientFactory = @MainActor (ServerSettings) throws -> any BackroomClientProtocol

    /// Most recently viewed first; where to go when the open buffer goes.
    @ObservationIgnored private var recent: [BufferID] = []
    /// A channel the user just /joined; selected once it exists.
    @ObservationIgnored private var pendingSelection: BufferID?
    @ObservationIgnored private let defaults: UserDefaults
    private static let lastBufferKey = "lastBuffer"

    @ObservationIgnored private var client: (any BackroomClientProtocol)?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private let makeClient: ClientFactory
    @ObservationIgnored private let authenticator: any WebAuthenticator
    /// Whether the app is frontmost; buffers are only marked read then.
    @ObservationIgnored var isActive = true {
        didSet { if isActive, let selection { markRead(selection) } }
    }

    init(settings: ServerSettings, authenticator: any WebAuthenticator,
         defaults: UserDefaults = .standard, makeClient: @escaping ClientFactory) {
        self.settings = settings
        self.defaults = defaults
        self.authenticator = authenticator
        self.makeClient = makeClient
    }

    // MARK: Lifecycle

    /// Creates the client for the current settings and connects if a login
    /// is saved.
    func start() async {
        stopSession()
        do {
            let client = try makeClient(settings)
            self.client = client
            if try client.hasLogin() {
                await connect()
            } else {
                phase = .loggedOut
            }
        } catch {
            phase = .failed(Self.describe(error))
        }
    }

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
        new.save()
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
        selection = nil
        recent = []
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
                notice = "Your login was rejected (\(reason)). Log in again."
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
            if selection?.network == id { selection = nil }
        case .bufferAdded(let network, let name, let kind):
            ensureBuffer(network: network, name: name, kind: kind)
        case .bufferRemoved(let network, let name):
            let id = BufferID(network: network, name: name)
            buffers[id] = nil
            recent.removeAll { $0 == id }
            if let i = networkIndex(network) { networks[i].buffers.removeAll { $0 == id } }
            if selection == id {
                // Back to the previous conversation, else the network's server buffer.
                let fallback = recent.first { buffers[$0] != nil }
                    ?? networkIndex(network).flatMap { i in
                        networks[i].buffers.first { buffers[$0]?.kind == .server }
                    }
                select(fallback)
            }
        case .topic(let network, let channel, let topic):
            buffers[BufferID(network: network, name: channel)]?.topic = topic
        case .members(let network, let channel, let list):
            buffers[BufferID(network: network, name: channel)]?.members = list
        case .message(let network, let name, let message):
            let id = ensureBuffer(network: network, name: name, kind: nil)
            buffers[id]?.merge([message])
            if id == selection { markRead(id) }
        // History and read markers only update buffers that still exist:
        // they can arrive after we left a channel, and mustn't bring it back.
        case .history(let network, let name, let messages):
            let id = BufferID(network: network, name: name)
            guard let buffer = buffers[id] else { return }
            buffer.merge(messages)
            buffer.historyLoading = false
            if messages.count < Self.historyPage { buffer.historyExhausted = true }
            if id == selection { markRead(id) }
        case .readMarker(let network, let name, let time):
            // Markers only move forward: soju's reply to the initial query
            // can arrive after this device already marked the buffer read.
            guard let buffer = buffers[BufferID(network: network, name: name)] else { return }
            if let time, buffer.readMarker.map({ time > $0 }) ?? true {
                buffer.readMarker = time
            }
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
            buffers[id] = BufferModel(id: id, name: name, kind: kind)
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
            } else if selection == nil, id == savedSelection {
                // Reopen what was open last time, once it exists again.
                select(id)
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

    func select(_ id: BufferID?) {
        selection = id
        guard let id, let buffer = buffers[id], let client else { return }
        recent.removeAll { $0 == id }
        recent.insert(id, at: 0)
        defaults.set([id.network, id.key], forKey: Self.lastBufferKey)
        if !buffer.historyRequested {
            buffers[id]?.historyRequested = true
            buffers[id]?.historyLoading = true
            run { try client.loadHistory(network: id.network, target: buffer.name, before: nil, limit: Self.historyPage) }
        }
        markRead(id)
    }

    private var savedSelection: BufferID? {
        guard let saved = defaults.stringArray(forKey: Self.lastBufferKey), saved.count == 2 else { return nil }
        return BufferID(network: saved[0], name: saved[1])
    }

    var selectedBuffer: BufferModel? { selection.flatMap { buffers[$0] } }

    /// Opens (or switches to) a private conversation with `nick`.
    func openQuery(network: String, nick: String) {
        let id = ensureBuffer(network: network, name: nick, kind: .query)
        select(id)
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
            !buffer.historyExhausted, let oldest = buffer.messages.first
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
        case .join(let channel):
            // Switch to it: now if we're already there, else once joined.
            let target = BufferID(network: id.network, name: String(channel.split(separator: ",").first ?? ""))
            if buffers[target] != nil {
                select(target)
            } else {
                pendingSelection = target
            }
            return run { try client.join(network: id.network, channel: channel) }
        case .part(let channel, let reason):
            return run { try client.part(network: id.network, channel: channel, reason: reason) }
        case .query(let nick, let text):
            openQuery(network: id.network, nick: nick)
            guard let text else { return true }
            return run { try client.sendMessage(network: id.network, target: nick, text: text) }
        case .raw(let line):
            return run { try client.sendRaw(network: id.network, line: line) }
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
