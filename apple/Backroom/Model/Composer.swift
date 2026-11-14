import Foundation

/// What a line typed into the composer asks for.
enum ComposerAction: Equatable {
    case message(String)
    case action(String)
    case join(String)
    case part(channel: String, reason: String?)
    case invalid(String)
}

enum Composer {
    /// Parses one line. `buffer` is the current buffer, the default for
    /// `/part`. Returns nil for blank input.
    static func parse(_ input: String, buffer: String) -> ComposerAction? {
        let line = input.trimmingCharacters(in: .newlines)
        guard !line.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        guard line.hasPrefix("/") else { return .message(line) }
        // `//text` sends `/text` literally.
        if line.hasPrefix("//") { return .message(String(line.dropFirst())) }

        let body = line.dropFirst()
        let parts = body.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        let command = parts.first.map { $0.lowercased() } ?? ""
        let rest = parts.count > 1 ? String(parts[1]).trimmingCharacters(in: .whitespaces) : ""

        switch command {
        case "me":
            return rest.isEmpty ? .invalid("Usage: /me <action>") : .action(rest)
        case "join", "j":
            guard let channel = rest.split(separator: " ").first else {
                return .invalid("Usage: /join <#channel>")
            }
            return .join(String(channel))
        case "part", "leave":
            let words = rest.split(separator: " ", maxSplits: 1)
            if let first = words.first, first.first.map({ "#&+!".contains($0) }) == true {
                let reason = words.count > 1 ? String(words[1]) : nil
                return .part(channel: String(first), reason: reason)
            }
            return .part(channel: buffer, reason: rest.isEmpty ? nil : rest)
        default:
            return .invalid("Unknown command /\(command). Start with // to send a slash.")
        }
    }

    /// IRC text for a CTCP ACTION.
    static func actionText(_ text: String) -> String { "\u{1}ACTION \(text)\u{1}" }
}
