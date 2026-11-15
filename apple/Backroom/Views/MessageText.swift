import BackroomCore
import AppKit
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
    private static let linkSchemes: Set<String> = ["http", "https", "mailto", "ftp", "irc", "ircs"]

    /// Web links and channel names in plain (unformatted) text, as ranges
    /// into it.
    static func links(in plain: String) -> [(NSRange, URL)] {
        let range = NSRange(plain.startIndex..., in: plain)
        var links: [(NSRange, URL)] = []
        for match in detector?.matches(in: plain, range: range) ?? [] {
            if let url = match.url, linkSchemes.contains(url.scheme?.lowercased() ?? "") {
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
        return links.sorted { $0.0.location < $1.0.location }
    }

    /// The first web link in a message, for its preview.
    static func firstWebLink(_ text: String) -> URL? {
        links(in: stripFormatting(text: text)).first { ["http", "https"].contains($0.1.scheme?.lowercased()) }?.1
    }

    /// Links look like the text around them (the topic's are opened with ⌘).
    private static func addLinks(_ text: inout AttributedString, theme: Theme) {
        let plain = String(text.characters)
        for (nsRange, url) in links(in: plain) {
            guard let r = Range(nsRange, in: plain) else { continue }
            let start = plain.distance(from: plain.startIndex, to: r.lowerBound)
            let length = plain.distance(from: r.lowerBound, to: r.upperBound)
            let lower = text.index(text.startIndex, offsetByCharacters: start)
            let upper = text.index(lower, offsetByCharacters: length)
            text[lower..<upper].link = url
        }
    }

    /// Message text for `ChatTextView`: formatting in `font`, links marked
    /// with `.chatLink`.
    static func nsAttributed(
        _ text: String, formatting: Bool, theme: Theme, font: NSFont, color: NSColor, italic: Bool = false
    ) -> NSMutableAttributedString {
        let spans = formatting
            ? formatSpans(text: text)
            : [FormattedSpan(
                text: stripFormatting(text: text), bold: false, italic: false, underline: false,
                strikethrough: false, monospace: false, reverse: false, fg: nil, bg: nil)]
        let result = NSMutableAttributedString()
        for span in spans {
            var attributes: [NSAttributedString.Key: Any] = [
                .font: styled(font, bold: span.bold, italic: italic || span.italic, monospace: span.monospace),
                .foregroundColor: color,
            ]
            if span.underline { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            if span.strikethrough { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            var fg = span.fg.map(NSColor.init(rgb:))
            var bg = span.bg.map(NSColor.init(rgb:))
            if span.reverse {
                (fg, bg) = (bg ?? theme.nsBackgroundColor, fg ?? theme.nsTextColor)
            }
            if let fg { attributes[.foregroundColor] = fg }
            if let bg { attributes[.backgroundColor] = bg }
            result.append(NSAttributedString(string: span.text, attributes: attributes))
        }
        for (range, url) in links(in: result.string) {
            result.addAttribute(.chatLink, value: url, range: range)
        }
        return result
    }

    /// A plain run (nick, prefix) in `font`.
    static func run(
        _ text: String, font: NSFont, color: NSColor, bold: Bool = false, italic: Bool = false
    ) -> NSAttributedString {
        NSAttributedString(string: text, attributes: [
            .font: styled(font, bold: bold, italic: italic, monospace: false), .foregroundColor: color,
        ])
    }

    static func styled(_ font: NSFont, bold: Bool, italic: Bool, monospace: Bool) -> NSFont {
        var font = monospace ? NSFont.monospacedSystemFont(ofSize: font.pointSize, weight: .regular) : font
        let manager = NSFontManager.shared
        if bold { font = manager.convert(font, toHaveTrait: .boldFontMask) }
        if italic {
            let converted = manager.convert(font, toHaveTrait: .italicFontMask)
            // The system font has no italic face in the font manager's eyes.
            font = converted.fontDescriptor.symbolicTraits.contains(.italic)
                ? converted
                : NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(
                    font.fontDescriptor.symbolicTraits.union(.italic)), size: font.pointSize) ?? font
        }
        return font
    }

    /// The channel a join link points at.
    static func channel(from url: URL) -> String? {
        guard url.scheme == joinScheme else { return nil }
        let name = URLComponents(url: url, resolvingAgainstBaseURL: false)?.path ?? ""
        return name.isEmpty ? nil : name
    }
}

extension NSColor {
    /// `0xRRGGBB`.
    convenience init(rgb: UInt32) {
        self.init(
            srgbRed: CGFloat((rgb >> 16) & 0xff) / 255, green: CGFloat((rgb >> 8) & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255, alpha: 1)
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
