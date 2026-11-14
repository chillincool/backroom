import Foundation

/// What a line typed into the composer asks for.
enum ComposerAction: Equatable {
    case message(String)
    case action(String)
    /// Channels (comma-separated) and an optional key.
    case join(String, key: String?)
    case part(channel: String, reason: String?)
    /// Open a private conversation, optionally sending a first message.
    case query(nick: String, text: String?)
    /// Sent to the server as is.
    case raw(String)
    case invalid(String)
}

enum Composer {
    private static let channelPrefixes = "#&+!"

    /// Parses one line. `buffer` is the current buffer, the default target
    /// of `/part` and `/topic`. Returns nil for blank input.
    static func parse(_ input: String, buffer: String) -> ComposerAction? {
        let line = input.trimmingCharacters(in: .newlines)
        guard !line.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        guard line.hasPrefix("/") else { return .message(line) }
        // `//text` sends `/text` literally.
        if line.hasPrefix("//") { return .message(String(line.dropFirst())) }

        let parts = line.dropFirst().split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        let command = parts.first.map { $0.lowercased() } ?? ""
        let rest = parts.count > 1 ? String(parts[1]).trimmingCharacters(in: .whitespaces) : ""
        let (first, remainder) = splitWord(rest)

        switch command {
        case "me":
            return rest.isEmpty ? .invalid("Usage: /me <action>") : .action(rest)
        case "join", "j":
            guard let first else { return .invalid("Usage: /join <#channel> [key]") }
            let channels = withHashes(first)
            // The key is one word; anything after it is ignored.
            let key = remainder.flatMap { splitWord($0).0 }
            return .join(channels, key: key)
        case "part", "p", "leave":
            if let first, channelPrefixes.contains(first.first!) {
                return .part(channel: first, reason: remainder)
            }
            return .part(channel: buffer, reason: rest.isEmpty ? nil : rest)
        case "msg", "m":
            guard let first, let remainder else { return .invalid("Usage: /msg <nick> <message>") }
            return .query(nick: first, text: remainder)
        case "query", "q":
            guard let first else { return .invalid("Usage: /query <nick> [message]") }
            return .query(nick: first, text: remainder)
        case "topic", "t":
            return .raw(rest.isEmpty ? "TOPIC \(buffer)" : "TOPIC \(buffer) :\(rest)")
        case "quote", "raw":
            return rest.isEmpty ? .invalid("Usage: /quote <IRC line>") : .raw(rest)
        case "":
            return .invalid("Type a command after the slash, or start with // to send a slash.")
        default:
            // Anything else goes to the server as is: /whois, /nick, /away, …
            return .raw(rest.isEmpty ? command.uppercased() : "\(command.uppercased()) \(rest)")
        }
    }

    /// `/j foo` means #foo, as in most clients.
    static func withHashes(_ names: String) -> String {
        names.split(separator: ",").map { name in
            channelPrefixes.contains(name.first!) ? String(name) : "#" + name
        }.joined(separator: ",")
    }

    /// The first word and what follows it (nil if nothing does).
    private static func splitWord(_ text: String) -> (String?, String?) {
        let words = text.split(separator: " ", maxSplits: 1)
        let rest = words.count > 1 ? String(words[1]).trimmingCharacters(in: .whitespaces) : nil
        return (words.first.map(String.init), rest?.isEmpty == true ? nil : rest)
    }

    /// IRC text for a CTCP ACTION.
    static func actionText(_ text: String) -> String { "\u{1}ACTION \(text)\u{1}" }
}
