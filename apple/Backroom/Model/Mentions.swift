import BackroomCore
import Foundation

/// Finds our nick and highlight words in message text (whole words, any
/// case), and knows whom to ignore.
struct MentionMatcher: Equatable {
    private(set) var terms: [String] = []
    private(set) var ignored: Set<String> = []
    private var regex: NSRegularExpression?

    init(nick: String? = nil, words: [String] = [], ignored: [String] = []) {
        terms = ([nick].compactMap { $0 } + words)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        self.ignored = Set(ignored.map { $0.lowercased() })
        guard !terms.isEmpty else { return }
        // Not inside a longer word: "bob" matches "bob:" and "@bob", not "bobby".
        let alternatives = terms.map(NSRegularExpression.escapedPattern(for:)).joined(separator: "|")
        regex = try? NSRegularExpression(
            pattern: "(?<![\\w\\[\\]\\\\`^{|}-])(?:\(alternatives))(?![\\w\\[\\]\\\\`^{|}-])",
            options: [.caseInsensitive])
    }

    func matches(_ text: String) -> Bool {
        guard let regex else { return false }
        let plain = stripFormatting(text: text)
        return regex.firstMatch(in: plain, range: NSRange(plain.startIndex..., in: plain)) != nil
    }

    func isIgnored(_ nick: String) -> Bool { ignored.contains(nick.lowercased()) }

    static func == (a: Self, b: Self) -> Bool { a.terms == b.terms && a.ignored == b.ignored }
}
