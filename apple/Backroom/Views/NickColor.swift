import SwiftUI

/// Nick colours: by channel rank (as IRC clients and Discord roles do), by
/// nick, or none, in the theme's colours.
enum NickColor {
    /// `prefix` is the member's prefixes, highest first; nil if unknown.
    /// The nick with its highest prefix in front, IRC style: `@alice`.
    static func label(_ nick: String, prefix: String?) -> String {
        (prefix?.first.map(String.init) ?? "") + nick
    }

    static func color(nick: String, prefix: String?, theme: Theme, coloring: NickColoring) -> Color {
        switch coloring {
        case .rank: theme.rankColor(prefix?.first)
        case .nick: theme.nickPalette[paletteIndex(nick, count: theme.nickPalette.count)]
        case .none: theme.textColor
        }
    }

    /// Stable across launches (unlike `hashValue`) and case-insensitive.
    static func paletteIndex(_ nick: String, count: Int) -> Int {
        var hash: UInt32 = 5381
        for byte in nick.lowercased().utf8 { hash = hash &* 33 &+ UInt32(byte) }
        return Int(hash % UInt32(max(count, 1)))
    }
}
