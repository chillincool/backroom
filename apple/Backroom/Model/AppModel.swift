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
    private(set) var buffers: [BufferID: BufferState] = [:]
    private(set) var selection: BufferID?
    /// The latest error worth showing; the view clears it.
    var notice: String?

    static let historyPage: UInt32 = 100

    typealias ClientFactory = @MainActor (ServerSettings) throws -> any BackroomClientProtocol

    @ObservationIgnored private var client: (any BackroomClientProtocol)?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private let makeClient: ClientFactory
    @ObservationIgnored private let authenticator: any WebAuthenticator
    /// Whether the app is frontmost; buffers are only marked read then.
    @ObservationIgnored var isActive = true {
        didSet { if isActive, let selection { markRead(selection) } }
    }

    init(settings: ServerSettings, authenticator: any WebAuthenticator, makeClient: @escaping ClientFactory) {
        self.settings = settings
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
            if selection?.network == id { selection = nil }
        case .bufferAdded(let network, let name, let kind):
            ensureBuffer(network: network, name: name, kind: kind)
        case .message(let network, let name, let message):
            let id = ensureBuffer(network: network, name: name, kind: nil)
            buffers[id]?.merge([message])
            if id == selection { markRead(id) }
        case .history(let network, let name, let messages):
            let id = ensureBuffer(network: network, name: name, kind: nil)
            buffers[id]?.merge(messages)
            buffers[id]?.historyLoading = false
            if messages.count < Self.historyPage { buffers[id]?.historyExhausted = true }
            if id == selection { markRead(id) }
        case .readMarker(let network, let name, let time):
            // Markers only move forward: soju's reply to the initial query
            // can arrive after this device already marked the buffer read.
            let id = ensureBuffer(network: network, name: name, kind: nil)
            if let time, buffers[id]?.readMarker.map({ time > $0 }) ?? true {
                buffers[id]?.readMarker = time
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
            buffers[id] = BufferState(id: id, name: name, kind: kind)
            if let i = networkIndex(network) {
                // The server buffer goes first.
                if kind == .server {
                    networks[i].buffers.insert(id, at: 0)
                } else {
                    networks[i].buffers.append(id)
                }
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
        if !buffer.historyRequested {
            buffers[id]?.historyRequested = true
            buffers[id]?.historyLoading = true
            run { try client.loadHistory(network: id.network, target: buffer.name, before: nil, limit: Self.historyPage) }
        }
        markRead(id)
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
            return run { try client.join(network: id.network, channel: channel) }
        case .part(let channel, let reason):
            return run { try client.part(network: id.network, channel: channel, reason: reason) }
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
