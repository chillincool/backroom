import SwiftUI

/// Nick colours by channel status, as IRC clients (and Discord roles) do.
enum NickColor {
    /// `prefix` is the member's prefixes, highest first; nil if unknown.
    /// The nick with its highest prefix in front, IRC style: `@alice`.
    static func label(_ nick: String, prefix: String?) -> String {
        (prefix?.first.map(String.init) ?? "") + nick
    }

    static func color(prefix: String?) -> Color {
        switch prefix?.first {
        case "~", "&": .purple
        case "@": .green
        case "%": .teal
        case "+": .orange
        case nil: .primary
        default: .indigo  // ranks some networks add, like IRCnet's !
        }
    }
}
