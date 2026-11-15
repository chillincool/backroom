import AppKit
import SwiftUI

/// Colours for the chat area, member list and topic. Each colour is a
/// `#rrggbb` string; nil means the system colour for that role, so the
/// default themes look native and follow the system accent.
struct Theme: Codable, Identifiable, Hashable, Sendable {
    var id: String
    var name: String
    /// Which appearance the theme is for: it's offered as a dark or a light theme.
    var isDark: Bool
    /// Chat background.
    var background: String?
    /// Topic banner, day separators, composer.
    var surface: String?
    var text: String?
    /// Timestamps, joins and parts, notices.
    var muted: String?
    /// Your own nick.
    var accent: String?
    /// Nick colours by channel rank.
    var ranks = RankColors()
    /// Nick colours when colouring by nick; empty for the system palette.
    var nicks: [String] = []

    struct RankColors: Codable, Hashable, Sendable {
        var owner: String?
        var admin: String?
        var op: String?
        var halfop: String?
        var voice: String?
    }

    var isBuiltIn: Bool { id.hasPrefix(Self.builtInPrefix) }

    /// A theme file: one theme or an array of them.
    static func decodeAll(_ data: Data) throws -> [Theme] {
        let decoder = JSONDecoder()
        if let themes = try? decoder.decode([Theme].self, from: data) { return themes }
        return [try decoder.decode(Theme.self, from: data)]
    }
    static let builtInPrefix = "builtin."

    // MARK: Resolved colours

    var backgroundColor: Color { Color(hex: background) ?? Color(nsColor: .textBackgroundColor) }
    var surfaceColor: Color { Color(hex: surface) ?? Color(nsColor: .windowBackgroundColor) }
    var textColor: Color { Color(hex: text) ?? .primary }
    var mutedColor: Color { Color(hex: muted) ?? .secondary }
    var accentColor: Color { Color(hex: accent) ?? .accentColor }
    /// AppKit versions, for message text; system colours stay dynamic.
    var nsBackgroundColor: NSColor { Color(hex: background).map { NSColor($0) } ?? .textBackgroundColor }
    var nsTextColor: NSColor { Color(hex: text).map { NSColor($0) } ?? .labelColor }
    var nsMutedColor: NSColor { Color(hex: muted).map { NSColor($0) } ?? .secondaryLabelColor }
    var nsAccentColor: NSColor { Color(hex: accent).map { NSColor($0) } ?? .controlAccentColor }

    func rankColor(_ symbol: Character?) -> Color {
        switch symbol {
        case "~": Color(hex: ranks.owner) ?? .purple
        case "&": Color(hex: ranks.admin) ?? .pink
        case "@": Color(hex: ranks.op) ?? .green
        case "%": Color(hex: ranks.halfop) ?? .teal
        case "+": Color(hex: ranks.voice) ?? .orange
        case nil: textColor
        default: Color(hex: ranks.admin) ?? .indigo  // ranks some networks add, like IRCnet's !
        }
    }

    /// Readable on both light and dark backgrounds; no yellow.
    static let systemNickPalette: [Color] = [.red, .orange, .green, .mint, .teal, .cyan, .blue, .indigo, .purple, .pink, .brown]

    var nickPalette: [Color] {
        let colors = nicks.compactMap { Color(hex: $0) }
        return colors.isEmpty ? Self.systemNickPalette : colors
    }
}

// MARK: Built-in themes

extension Theme {
    static let defaultLight = Theme(id: "builtin.default-light", name: "Default", isDark: false)
    static let defaultDark = Theme(id: "builtin.default-dark", name: "Default", isDark: true)

    static let builtIns: [Theme] = [
        defaultLight, defaultDark,
        solarizedLight, solarizedDark,
        gruvboxLight, gruvboxDark,
        dracula,
        nordLight, nord,
        highContrastLight, highContrastDark,
    ]

    // https://ethanschoonover.com/solarized/
    private static let solarizedAccents = (
        yellow: "#b58900", orange: "#cb4b16", red: "#dc322f", magenta: "#d33682",
        violet: "#6c71c4", blue: "#268bd2", cyan: "#2aa198", green: "#859900")
    private static var solarizedRanks: RankColors {
        let s = solarizedAccents
        return RankColors(owner: s.violet, admin: s.magenta, op: s.green, halfop: s.cyan, voice: s.orange)
    }
    private static var solarizedNicks: [String] {
        let s = solarizedAccents
        return [s.yellow, s.orange, s.red, s.magenta, s.violet, s.blue, s.cyan, s.green]
    }
    static let solarizedLight = Theme(
        id: "builtin.solarized-light", name: "Solarized", isDark: false,
        background: "#fdf6e3", surface: "#eee8d5", text: "#657b83", muted: "#93a1a1", accent: "#268bd2",
        ranks: solarizedRanks, nicks: solarizedNicks)
    static let solarizedDark = Theme(
        id: "builtin.solarized-dark", name: "Solarized", isDark: true,
        background: "#002b36", surface: "#073642", text: "#839496", muted: "#586e75", accent: "#268bd2",
        ranks: solarizedRanks, nicks: solarizedNicks)

    // https://github.com/morhetz/gruvbox
    static let gruvboxLight = Theme(
        id: "builtin.gruvbox-light", name: "Gruvbox", isDark: false,
        background: "#fbf1c7", surface: "#ebdbb2", text: "#3c3836", muted: "#928374", accent: "#076678",
        ranks: RankColors(owner: "#8f3f71", admin: "#9d0006", op: "#79740e", halfop: "#427b58", voice: "#af3a03"),
        nicks: ["#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#af3a03"])
    static let gruvboxDark = Theme(
        id: "builtin.gruvbox-dark", name: "Gruvbox", isDark: true,
        background: "#282828", surface: "#3c3836", text: "#ebdbb2", muted: "#928374", accent: "#83a598",
        ranks: RankColors(owner: "#d3869b", admin: "#fb4934", op: "#b8bb26", halfop: "#8ec07c", voice: "#fe8019"),
        nicks: ["#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#fe8019"])

    // https://draculatheme.com/contribute (dark only)
    static let dracula = Theme(
        id: "builtin.dracula", name: "Dracula", isDark: true,
        background: "#282a36", surface: "#44475a", text: "#f8f8f2", muted: "#6272a4", accent: "#bd93f9",
        ranks: RankColors(owner: "#ff79c6", admin: "#ff5555", op: "#50fa7b", halfop: "#8be9fd", voice: "#ffb86c"),
        nicks: ["#8be9fd", "#50fa7b", "#ffb86c", "#ff79c6", "#bd93f9", "#ff5555", "#f1fa8c"])

    // https://www.nordtheme.com/docs/colors-and-palettes
    private static let nordRanks = RankColors(
        owner: "#b48ead", admin: "#bf616a", op: "#a3be8c", halfop: "#8fbcbb", voice: "#d08770")
    static let nord = Theme(
        id: "builtin.nord", name: "Nord", isDark: true,
        background: "#2e3440", surface: "#3b4252", text: "#d8dee9", muted: "#7b88a1", accent: "#88c0d0",
        ranks: nordRanks,
        nicks: ["#bf616a", "#d08770", "#ebcb8b", "#a3be8c", "#8fbcbb", "#88c0d0", "#81a1c1", "#b48ead"])
    static let nordLight = Theme(
        id: "builtin.nord-light", name: "Nord", isDark: false,
        background: "#eceff4", surface: "#e5e9f0", text: "#2e3440", muted: "#4c566a", accent: "#5e81ac",
        ranks: RankColors(owner: "#8a5f87", admin: "#bf616a", op: "#5f7e45", halfop: "#3f7f7d", voice: "#b5643e"),
        nicks: ["#bf616a", "#b5643e", "#8a6d1e", "#5f7e45", "#3f7f7d", "#5e81ac", "#8a5f87"])

    static let highContrastLight = Theme(
        id: "builtin.high-contrast-light", name: "High Contrast", isDark: false,
        background: "#ffffff", surface: "#e6e6e6", text: "#000000", muted: "#4d4d4d", accent: "#0040dd",
        ranks: RankColors(owner: "#7a00cc", admin: "#b00060", op: "#006b00", halfop: "#00676b", voice: "#8a4b00"),
        nicks: ["#b30000", "#8a4b00", "#006b00", "#00676b", "#0040dd", "#7a00cc", "#b00060"])
    static let highContrastDark = Theme(
        id: "builtin.high-contrast-dark", name: "High Contrast", isDark: true,
        background: "#000000", surface: "#262626", text: "#ffffff", muted: "#bfbfbf", accent: "#4da3ff",
        ranks: RankColors(owner: "#d699ff", admin: "#ff7ab8", op: "#5cff5c", halfop: "#4dffff", voice: "#ffc04d"),
        nicks: ["#ff6b6b", "#ffc04d", "#5cff5c", "#4dffff", "#4da3ff", "#d699ff", "#ff7ab8"])
}

// MARK: Hex colours

extension Color {
    /// `#rrggbb` (or `rrggbb`); nil for nil or anything else.
    init?(hex: String?) {
        guard var hex = hex?.trimmingCharacters(in: .whitespaces) else { return nil }
        if hex.hasPrefix("#") { hex.removeFirst() }
        guard hex.count == 6, let value = UInt32(hex, radix: 16) else { return nil }
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255)
    }

    /// `#rrggbb` in sRGB, resolving dynamic colours for `appearance`.
    func hex(dark: Bool) -> String {
        var color = NSColor(self)
        NSAppearance(named: dark ? .darkAqua : .aqua)?.performAsCurrentDrawingAppearance {
            color = color.usingColorSpace(.sRGB) ?? color
        }
        guard let rgb = color.usingColorSpace(.sRGB) else { return "#000000" }
        let c = [rgb.redComponent, rgb.greenComponent, rgb.blueComponent]
            .map { Int(($0 * 255).rounded()).clamped(0, 255) }
        return String(format: "#%02x%02x%02x", c[0], c[1], c[2])
    }
}

private extension Int {
    func clamped(_ low: Int, _ high: Int) -> Int { Swift.min(Swift.max(self, low), high) }
}

extension EnvironmentValues {
    /// The theme for the current appearance.
    @Entry var theme: Theme = .defaultLight
}
