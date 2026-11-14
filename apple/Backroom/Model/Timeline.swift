import BackroomCore
import Foundation

/// What a buffer's message list shows: messages, with each run of joins and
/// parts folded into one line.
enum TimelineItem: Identifiable {
    case message(ChatMessage)
    case presence([ChatMessage])

    var id: String {
        switch self {
        case .message(let m): m.stableID
        case .presence(let run): "presence-" + (run.first?.stableID ?? "")
        }
    }

    static func build(_ messages: [ChatMessage]) -> [TimelineItem] {
        var items: [TimelineItem] = []
        var run: [ChatMessage] = []
        for message in messages {
            if message.kind == .join || message.kind == .part {
                run.append(message)
                continue
            }
            if !run.isEmpty {
                items.append(.presence(run))
                run = []
            }
            items.append(.message(message))
        }
        if !run.isEmpty { items.append(.presence(run)) }
        return items
    }

    /// "→ alice, bob joined · ← carol left", long lists shortened.
    static func summary(_ run: [ChatMessage]) -> String {
        func names(_ kind: MessageKind) -> [String] {
            var seen = Set<String>()
            return run.filter { $0.kind == kind }.map(\.sender).filter { seen.insert($0.lowercased()).inserted }
        }
        func list(_ names: [String]) -> String {
            let shown = 4
            guard names.count > shown + 1 else {
                return names.count < 2 ? names.joined()
                    : names.dropLast().joined(separator: ", ") + " and " + names.last!
            }
            return names.prefix(shown).joined(separator: ", ") + " and \(names.count - shown) others"
        }
        var parts: [String] = []
        let joined = names(.join), left = names(.part)
        if !joined.isEmpty { parts.append("→ \(list(joined)) joined") }
        if !left.isEmpty { parts.append("← \(list(left)) left") }
        return parts.joined(separator: " · ")
    }
}

extension ChatMessage {
    /// Identity in lists: the server's msgid, else what the message is.
    var stableID: String {
        id ?? "\(time.timeIntervalSince1970)|\(sender)|\(kind)|\(text)"
    }
}
