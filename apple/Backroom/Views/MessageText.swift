import BackroomCore
import Foundation
import SwiftUI

/// Message text for display: mIRC formatting (or stripped of it), web
/// links, and channel names that join on click.
enum MessageText {
    /// Channel links use this scheme; `BufferView` handles them.
    static let joinScheme = "backroom-join"

    static func attributed(_ text: String, formatting: Bool, theme: Theme) -> AttributedString {
        let spans = formatting
            ? formatSpans(text: text)
            : [FormattedSpan(
                text: stripFormatting(text: text), bold: false, italic: false, underline: false,
                strikethrough: false, monospace: false, reverse: false, fg: nil, bg: nil)]
        var result = AttributedString()
        for span in spans {
            var part = AttributedString(span.text)
            var intent: InlinePresentationIntent = []
            if span.bold { intent.insert(.stronglyEmphasized) }
            if span.italic { intent.insert(.emphasized) }
            if span.monospace { intent.insert(.code) }
            if span.strikethrough { intent.insert(.strikethrough) }
            if !intent.isEmpty { part.inlinePresentationIntent = intent }
            if span.underline { part.underlineStyle = .single }
            var fg = span.fg.map(Color.init(rgb:))
            var bg = span.bg.map(Color.init(rgb:))
            if span.reverse {
                (fg, bg) = (bg ?? theme.backgroundColor, fg ?? theme.textColor)
            }
            if let fg { part.foregroundColor = fg }
            if let bg { part.backgroundColor = bg }
            result += part
        }
        addLinks(&result, theme: theme)
        return result
    }

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
    /// `#channel` at the start or after a space or bracket.
    private static let channelPattern = try? NSRegularExpression(pattern: "(?<=^|[\\s(\\[])#[^\\s,()\\[\\]\\x07]+")

    private static func addLinks(_ text: inout AttributedString, theme: Theme) {
        let plain = String(text.characters)
        let range = NSRange(plain.startIndex..., in: plain)
        var links: [(NSRange, URL)] = []
        for match in detector?.matches(in: plain, range: range) ?? [] {
            if let url = match.url, ["http", "https", "mailto", "ftp", "irc", "ircs"].contains(url.scheme?.lowercased() ?? "") {
                links.append((match.range, url))
            }
        }
        for match in channelPattern?.matches(in: plain, range: range) ?? [] {
            guard !links.contains(where: { NSIntersectionRange($0.0, match.range).length > 0 }),
                let r = Range(match.range, in: plain)
            else { continue }
            var components = URLComponents()
            components.scheme = joinScheme
            components.path = String(plain[r]).trimmingCharacters(in: CharacterSet(charactersIn: ".!?:;'\""))
            if let url = components.url { links.append((match.range, url)) }
        }
        for (nsRange, url) in links {
            guard let r = Range(nsRange, in: plain) else { continue }
            let start = plain.distance(from: plain.startIndex, to: r.lowerBound)
            let length = plain.distance(from: r.lowerBound, to: r.upperBound)
            let lower = text.index(text.startIndex, offsetByCharacters: start)
            let upper = text.index(lower, offsetByCharacters: length)
            text[lower..<upper].link = url
            text[lower..<upper].foregroundColor = theme.accentColor
            text[lower..<upper].underlineStyle = .single
        }
    }

    /// The channel a join link points at.
    static func channel(from url: URL) -> String? {
        guard url.scheme == joinScheme else { return nil }
        let name = URLComponents(url: url, resolvingAgainstBaseURL: false)?.path ?? ""
        return name.isEmpty ? nil : name
    }
}

extension Color {
    /// `0xRRGGBB`.
    init(rgb: UInt32) {
        self.init(
            .sRGB, red: Double((rgb >> 16) & 0xff) / 255, green: Double((rgb >> 8) & 0xff) / 255,
            blue: Double(rgb & 0xff) / 255)
    }
}
