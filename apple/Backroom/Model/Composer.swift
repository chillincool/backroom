import Foundation

/// What a line typed into the composer asks for.
enum ComposerAction: Equatable {
    case message(String)
    case action(String)
    /// Channels (comma-separated) and an optional key.
    case join(String, key: String?)
    case part(channel: String, reason: String?)
    /// Leave and rejoin.
    case cycle(channel: String)
    /// Open a private conversation, optionally sending a first message.
    case query(nick: String, text: String?)
    /// Open the channel browser.
    case listChannels
    /// Forget this buffer's messages on this device (until reloaded).
    case clear
    /// Leave the channel or close the conversation.
    case close
    case ignore(String)
    case unignore(String)
    /// Sent to the server as is.
    case raw(String)
    case invalid(String)
}

enum Composer {
    static let channelPrefixes = "#&+!"

    /// Every command, for help and completion.
    static let commands = [
        "away", "back", "ban", "clear", "close", "ctcp", "cycle", "deop", "devoice", "ignore",
        "invite", "join", "kick", "list", "me", "mode", "msg", "names", "nick", "notice", "op",
        "part", "query", "quote", "topic", "unban", "unignore", "voice", "whois",
    ]

    /// Parses one line. `buffer` is the current buffer, the default target
    /// of channel commands. Returns nil for blank input.
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
        let inChannel = isChannel(buffer)
        let needsChannel = ComposerAction.invalid("/\(command) works in a channel.")

        switch command {
        case "me":
            return rest.isEmpty ? .invalid("Usage: /me <action>") : .action(rest)
        case "join", "j":
            guard let first else { return .invalid("Usage: /join <#channel> [key]") }
            // The key is one word; anything after it is ignored.
            return .join(withHashes(first), key: remainder.flatMap { splitWord($0).0 })
        case "part", "p", "leave":
            if let first, isChannel(first) {
                return .part(channel: first, reason: remainder)
            }
            guard inChannel else { return needsChannel }
            return .part(channel: buffer, reason: rest.isEmpty ? nil : rest)
        case "cycle", "rejoin":
            return inChannel ? .cycle(channel: buffer) : needsChannel
        case "msg", "m":
            guard let first, let remainder else { return .invalid("Usage: /msg <nick> <message>") }
            return .query(nick: first, text: remainder)
        case "query", "q":
            guard let first else { return .invalid("Usage: /query <nick> [message]") }
            return .query(nick: first, text: remainder)
        case "notice":
            guard let first, let remainder else { return .invalid("Usage: /notice <target> <message>") }
            return .raw("NOTICE \(first) :\(remainder)")
        case "ctcp":
            guard let first, let remainder else { return .invalid("Usage: /ctcp <nick> <command> [arguments]") }
            let (ctcp, args) = splitWord(remainder)
            let body = ([ctcp?.uppercased()] + [args]).compactMap { $0 }.joined(separator: " ")
            return .raw("PRIVMSG \(first) :\u{1}\(body)\u{1}")
        case "topic", "t":
            guard inChannel else { return needsChannel }
            return .raw(rest.isEmpty ? "TOPIC \(buffer)" : "TOPIC \(buffer) :\(rest)")
        case "nick":
            guard let first else { return .invalid("Usage: /nick <new nick>") }
            return .raw("NICK \(first)")
        case "away":
            return .raw("AWAY :\(rest.isEmpty ? "Away" : rest)")
        case "back":
            return .raw("AWAY")
        case "whois", "wi":
            guard let first else { return .invalid("Usage: /whois <nick>") }
            return .raw("WHOIS \(first)")
        case "invite":
            guard let first else { return .invalid("Usage: /invite <nick> [#channel]") }
            if let channel = remainder.flatMap({ splitWord($0).0 }) { return .raw("INVITE \(first) \(channel)") }
            return inChannel ? .raw("INVITE \(first) \(buffer)") : needsChannel
        case "kick", "k":
            guard inChannel else { return needsChannel }
            guard let first else { return .invalid("Usage: /kick <nick> [reason]") }
            return .raw("KICK \(buffer) \(first)" + (remainder.map { " :\($0)" } ?? ""))
        case "ban", "unban":
            guard inChannel else { return needsChannel }
            guard let first else { return .invalid("Usage: /\(command) <nick or mask>") }
            return .raw("MODE \(buffer) \(command == "ban" ? "+" : "-")b \(banMask(first))")
        case "op", "deop", "voice", "devoice":
            guard inChannel else { return needsChannel }
            let nicks = rest.split(separator: " ").map(String.init)
            guard !nicks.isEmpty else { return .invalid("Usage: /\(command) <nick> [nick…]") }
            let sign = command.hasPrefix("de") ? "-" : "+"
            let letter = command.hasSuffix("op") ? "o" : "v"
            return .raw("MODE \(buffer) \(sign)\(String(repeating: letter, count: nicks.count)) \(nicks.joined(separator: " "))")
        case "mode":
            // `/mode +m` means this channel; `/mode #c +m` or `/mode me +i` as given.
            if let first, first.hasPrefix("+") || first.hasPrefix("-") {
                return inChannel ? .raw("MODE \(buffer) \(rest)") : needsChannel
            }
            if rest.isEmpty { return inChannel ? .raw("MODE \(buffer)") : needsChannel }
            return .raw("MODE \(rest)")
        case "names":
            guard inChannel || first != nil else { return needsChannel }
            return .raw("NAMES \(first ?? buffer)")
        case "list":
            return .listChannels
        case "clear":
            return .clear
        case "close":
            return .close
        case "ignore", "unignore":
            guard let first else { return .invalid("Usage: /\(command) <nick>") }
            return command == "ignore" ? .ignore(first) : .unignore(first)
        case "quote", "raw":
            return rest.isEmpty ? .invalid("Usage: /quote <IRC line>") : .raw(rest)
        case "":
            return .invalid("Type a command after the slash, or start with // to send a slash.")
        default:
            // Anything else goes to the server as is: /motd, /who, /oper, …
            return .raw(rest.isEmpty ? command.uppercased() : "\(command.uppercased()) \(rest)")
        }
    }

    static func isChannel(_ name: String) -> Bool {
        name.first.map { channelPrefixes.contains($0) } ?? false
    }

    /// `/j foo` means #foo, as in most clients.
    static func withHashes(_ names: String) -> String {
        names.split(separator: ",").map { name in
            channelPrefixes.contains(name.first!) ? String(name) : "#" + name
        }.joined(separator: ",")
    }

    /// A nick becomes `nick!*@*`; masks are used as given.
    static func banMask(_ target: String) -> String {
        target.contains("!") || target.contains("@") || target.contains("*") ? target : "\(target)!*@*"
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

/// Tab completion of nicks, channels and commands: the first press
/// completes the last word, further presses cycle through the matches.
struct Completion: Equatable {
    /// The line without the word being completed.
    let base: String
    let matches: [String]
    var index = 0
    /// What the line read after the last completion; any other edit ends it.
    var result: String

    /// Starts completing `line`'s last word, or returns nil if nothing fits.
    static func start(_ line: String, nicks: [String], channels: [String]) -> Completion? {
        let wordStart = line.lastIndex(of: " ").map { line.index(after: $0) } ?? line.startIndex
        let word = String(line[wordStart...])
        guard !word.isEmpty else { return nil }
        let base = String(line[..<wordStart])
        let candidates: [String]
        if base.isEmpty && word.hasPrefix("/") {
            candidates = Composer.commands.map { "/" + $0 }
        } else if word.first.map({ Composer.channelPrefixes.contains($0) }) == true {
            candidates = channels
        } else {
            candidates = nicks
        }
        let matches = candidates.filter { $0.lowercased().hasPrefix(word.lowercased()) && $0 != word }
        guard !matches.isEmpty else { return nil }
        var completion = Completion(base: base, matches: matches, result: "")
        completion.result = completion.line
        return completion
    }

    /// The line with the current match: "nick: " at the start of a line,
    /// otherwise followed by a space.
    var line: String {
        let match = matches[index]
        let atStart = base.isEmpty && !match.hasPrefix("/") && !match.first.map { Composer.channelPrefixes.contains($0) }!
        return base + match + (atStart ? ": " : " ")
    }

    mutating func next() {
        index = (index + 1) % matches.count
        result = line
    }
}
