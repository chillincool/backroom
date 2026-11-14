import BackroomCore
import Foundation

/// A buffer is identified by its network and case-folded name.
struct BufferID: Hashable, Sendable {
    let network: String
    let key: String

    init(network: String, name: String) {
        self.network = network
        self.key = name.lowercased()
    }
}

struct BufferState: Identifiable {
    let id: BufferID
    /// Name as the server spelled it; used as the IRC target.
    var name: String
    var kind: BufferKind
    var messages: [ChatMessage] = []
    /// Everything up to here has been read (on any device).
    var readMarker: Date?
    var historyRequested = false
    var historyLoading = false
    /// The server has no older messages.
    var historyExhausted = false

    /// Messages from others after the read marker.
    var unread: Int {
        messages.reduce(0) { count, m in
            let counts = !m.own && m.kind.isChat && (readMarker.map { m.time > $0 } ?? true)
            return counts ? count + 1 : count
        }
    }

    /// Adds messages, skipping ones already present (history and live
    /// messages overlap), keeping time order.
    mutating func merge(_ incoming: [ChatMessage]) {
        let known = Set(messages.compactMap(\.id))
        let fresh = incoming.filter { $0.id.map { !known.contains($0) } ?? true }
        guard !fresh.isEmpty else { return }
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
}

extension MessageKind {
    /// Counts towards unread (not joins, parts and the like).
    var isChat: Bool {
        switch self {
        case .privmsg, .notice, .action: true
        case .join, .part, .kick, .topic: false
        }
    }
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
