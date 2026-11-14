import BackroomCore
import Foundation
import Observation

/// A buffer is identified by its network and case-folded name.
struct BufferID: Hashable, Sendable {
    let network: String
    let key: String

    init(network: String, name: String) {
        self.network = network
        self.key = name.lowercased()
    }
}

/// One buffer's state. A class of its own so views observe just the
/// buffer they show: a message in one channel doesn't redraw the others.
@MainActor @Observable
final class BufferModel: Identifiable {
    let id: BufferID
    /// Name as the server spelled it; used as the IRC target.
    var name: String
    var kind: BufferKind
    private(set) var messages: [ChatMessage] = []
    /// Everything up to here has been read (on any device).
    var readMarker: Date?
    /// Where the read marker was when the user last came to this buffer:
    /// the "New" line goes above the first message after it.
    var unreadLine: Date?
    var historyRequested = false
    var historyLoading = false
    /// The server has no older messages.
    var historyExhausted = false
    /// The user cleared the buffer (`/clear`); older history isn't loaded.
    var cleared = false
    /// Channel topic, mIRC formatting included; nil if none.
    var topic: String?
    /// The topic the user closed the banner on; it returns if the topic changes.
    var dismissedTopic: String?
    /// Channel members, highest rank first.
    var members: [Member] = [] {
        didSet { prefixByNick = Dictionary(members.map { ($0.nick.lowercased(), $0.prefix) }) { a, _ in a } }
    }
    /// Membership prefix by case-folded nick, for colouring senders.
    private(set) var prefixByNick: [String: String] = [:]
    /// Finds mentions of our nick and highlight words; set by `AppModel`.
    var mentions = MentionMatcher()
    /// Lines sent from this buffer's composer, oldest first, for ↑/↓.
    @ObservationIgnored var sentHistory: [String] = []

    @ObservationIgnored private var knownIDs: Set<String> = []

    init(id: BufferID, name: String, kind: BufferKind) {
        self.id = id
        self.name = name
        self.kind = kind
    }

    /// Messages from others after the read marker.
    var unread: Int { unreadCounts.all }

    /// Unread messages that mention us (every unread message in a private
    /// conversation does).
    var unreadMentions: Int { unreadCounts.mentions }

    private var unreadCounts: (all: Int, mentions: Int) {
        var all = 0, mentioned = 0
        for m in messages.reversed() {
            if let marker = readMarker, m.time <= marker { break }
            guard !m.own, isChat(m), !mentions.isIgnored(m.sender) else { continue }
            all += 1
            if kind == .query || isMention(m) { mentioned += 1 }
        }
        return (all, mentioned)
    }

    /// Counts towards unread: chat, and invitations to us.
    func isChat(_ m: ChatMessage) -> Bool {
        m.kind.isChat || (m.kind == .invite && kind == .server)
    }

    /// Mentions our nick or a highlight word, or invites us.
    func isMention(_ m: ChatMessage) -> Bool {
        guard !m.own else { return false }
        switch m.kind {
        case .privmsg, .notice, .action: return mentions.matches(m.text)
        case .invite: return kind == .server
        default: return false
        }
    }

    /// Forgets the messages shown so far, on this device only.
    func clear() {
        messages = []
        cleared = true
    }

    /// Adds messages, skipping ones already present (history and live
    /// messages overlap), keeping time order.
    func merge(_ incoming: [ChatMessage]) {
        let fresh = incoming.filter { m in m.id.map { !knownIDs.contains($0) } ?? true }
        guard !fresh.isEmpty else { return }
        knownIDs.formUnion(fresh.compactMap(\.id))
        if let last = messages.last, fresh.allSatisfy({ $0.time >= last.time }) {
            messages.append(contentsOf: fresh)
        } else {
            messages = (messages + fresh).sorted { $0.time < $1.time }
        }
    }
}

struct NetworkState: Identifiable {
    var id: String { network.id }
    var network: Network
    var connection: ConnectionState = .connecting
    /// Buffer keys in the order they appeared; the server buffer first.
    var buffers: [BufferID] = []
    /// Our nick there, once registered.
    var nick: String?
    /// We're marked away there.
    var away = false
}

extension MessageKind {
    /// Counts towards unread (not joins, parts and the like).
    var isChat: Bool {
        switch self {
        case .privmsg, .notice, .action: true
        case .join, .part, .kick, .topic, .quit, .nick, .mode, .invite, .reply, .error: false
        }
    }

    /// Comings and goings, folded into one line when the user wants.
    var isPresence: Bool { self == .join || self == .part || self == .quit }
}

extension ConnectionState {
    var summary: String {
        switch self {
        case .connecting: "Connecting…"
        case .connected: "Connected"
        case .disconnected(let error, let retryIn):
            "Disconnected (\(error)); retrying in \(Int(retryIn.rounded())) s"
        case .authFailed(let reason): "Login rejected: \(reason)"
        }
    }

    var isConnected: Bool { if case .connected = self { true } else { false } }
}
