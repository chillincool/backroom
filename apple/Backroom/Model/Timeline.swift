import BackroomCore
import Foundation

/// What a buffer's message list shows: messages, joins, parts and quits
/// (alone, folded into runs or hidden), a separator where the day changes
/// and the New line. Ignored people's messages are left out.
enum TimelineItem: Identifiable, Equatable {
    /// `continued`: the previous line is a message from the same sender,
    /// so the nick can be left out.
    case message(ChatMessage, continued: Bool)
    case presence([ChatMessage])
    case day(Date)
    /// Above the first unread message.
    case unreadLine

    var id: String {
        switch self {
        case .message(let m, _): m.stableID
        case .presence(let run): "presence-" + (run.first?.stableID ?? "")
        case .day(let date): "day-\(date.timeIntervalSince1970)"
        case .unreadLine: "unread-line"
        }
    }

    /// A run of messages from one sender breaks after this long a pause.
    static let runGap: TimeInterval = 30 * 60

    static func build(
        _ messages: [ChatMessage], presence: PresenceStyle = .fold, unreadAfter: Date? = nil,
        isIgnored: (String) -> Bool = { _ in false }, calendar: Calendar = .current
    ) -> [TimelineItem] {
        var lineDue = unreadAfter != nil
        var items: [TimelineItem] = []
        var run: [ChatMessage] = []
        var day: Date?
        /// The last message shown with its sender, while a run can continue.
        var previous: ChatMessage?

        func flush() {
            if !run.isEmpty { items.append(.presence(run)) }
            run = []
        }

        for message in messages where message.own || !isIgnored(message.sender) {
            let start = calendar.startOfDay(for: message.time)
            if start != day {
                flush()
                // No separator above the first day: the header says where history starts.
                if day != nil { items.append(.day(start)) }
                day = start
                previous = nil
            }
            if lineDue, let after = unreadAfter, message.time > after, !message.own, message.kind.isChat {
                flush()
                items.append(.unreadLine)
                lineDue = false
                previous = nil
            }
            if message.kind.isPresence {
                switch presence {
                case .hide: continue
                case .fold: run.append(message)
                case .show: items.append(.message(message, continued: false))
                }
                previous = nil
                continue
            }
            flush()
            let continued = message.kind == .privmsg && previous.map {
                $0.kind == .privmsg && $0.sender.lowercased() == message.sender.lowercased()
                    && message.time.timeIntervalSince($0.time) < runGap
            } == true
            items.append(.message(message, continued: continued))
            previous = message
        }
        flush()
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
        let joined = names(.join)
        var seen = Set<String>()
        let left = run.filter { $0.kind == .part || $0.kind == .quit }.map(\.sender)
            .filter { seen.insert($0.lowercased()).inserted }
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
