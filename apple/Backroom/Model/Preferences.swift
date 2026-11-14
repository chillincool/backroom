import AppKit
import Observation
import SwiftUI

enum AppearanceMode: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: Self { self }
    var title: String {
        switch self {
        case .system: "System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }
}

enum MessageLayout: String, CaseIterable, Identifiable {
    /// Time, nick and text in aligned columns (irssi, WeeChat).
    case columns
    /// Time, then nick and text run together.
    case inline
    /// The nick above a group of messages.
    case stacked
    var id: Self { self }
    var title: String {
        switch self {
        case .columns: "Columns"
        case .inline: "Inline"
        case .stacked: "Stacked"
        }
    }
}

enum TimestampStyle: String, CaseIterable, Identifiable {
    case hidden, minutes, seconds
    var id: Self { self }
    var title: String {
        switch self {
        case .hidden: "Hidden"
        case .minutes: "Hours and minutes"
        case .seconds: "With seconds"
        }
    }
}

enum PresenceStyle: String, CaseIterable, Identifiable {
    /// One line per join or part.
    case show
    /// Each run of joins and parts on one line.
    case fold
    case hide
    var id: Self { self }
    var title: String {
        switch self {
        case .show: "Show each"
        case .fold: "Fold into one line"
        case .hide: "Hide"
        }
    }
}

enum NickColoring: String, CaseIterable, Identifiable {
    /// By channel rank: ops, voiced, …
    case rank
    /// Each nick its own colour.
    case nick
    case none
    var id: Self { self }
    var title: String {
        switch self {
        case .rank: "By rank"
        case .nick: "By nick"
        case .none: "None"
        }
    }
}

enum Density: String, CaseIterable, Identifiable {
    case compact, regular, comfortable
    var id: Self { self }
    var title: String { rawValue.capitalized }
    /// Vertical padding around each message line.
    var padding: CGFloat {
        switch self {
        case .compact: 0
        case .regular: 1
        case .comfortable: 3
        }
    }
}

/// Interface preferences, kept in UserDefaults. Nothing secret.
@MainActor @Observable
final class Preferences {
    var appearance: AppearanceMode { didSet { set(appearance.rawValue, "appearance"); applyAppearance() } }
    var lightThemeID: String { didSet { set(lightThemeID, "lightTheme") } }
    var darkThemeID: String { didSet { set(darkThemeID, "darkTheme") } }
    var customThemes: [Theme] { didSet { set(try? JSONEncoder().encode(customThemes), "customThemes") } }

    /// "" for the system font, `Self.monospaced` for the system monospaced
    /// font, otherwise a font family name.
    var fontFamily: String { didSet { set(fontFamily, "fontFamily") } }
    var fontSize: Double { didSet { set(fontSize, "fontSize") } }
    var density: Density { didSet { set(density.rawValue, "density") } }

    var layout: MessageLayout { didSet { set(layout.rawValue, "layout") } }
    /// Show the nick on every message, not just the first of a run.
    var repeatNick: Bool { didSet { set(repeatNick, "repeatNick") } }
    /// Width of the nick column, in characters.
    var nickColumnWidth: Int { didSet { set(nickColumnWidth, "nickColumnWidth") } }
    var timestamps: TimestampStyle { didSet { set(timestamps.rawValue, "timestamps") } }
    var presence: PresenceStyle { didSet { set(presence.rawValue, "presence") } }
    var nickColoring: NickColoring { didSet { set(nickColoring.rawValue, "nickColoring") } }
    /// Show mIRC colours and formatting; off strips them.
    var showFormatting: Bool { didSet { set(showFormatting, "showFormatting") } }
    /// Words that count as mentions, besides our nick.
    var highlightWords: [String] { didSet { set(highlightWords, "highlightWords") } }
    /// Nicks whose messages are hidden.
    var ignored: [String] { didSet { set(ignored, "ignored") } }

    static let monospaced = "monospaced"
    static let fontSizes: ClosedRange<Double> = 10...24
    static let nickColumnWidths: ClosedRange<Int> = 6...24

    @ObservationIgnored private let defaults: UserDefaults
    private nonisolated static let prefix = "prefs."

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        func string(_ key: String) -> String? { Self.read(defaults, key) }
        func value<E: RawRepresentable<String>>(_ key: String, _ fallback: E) -> E {
            Self.read(defaults, key).flatMap(E.init(rawValue:)) ?? fallback
        }
        appearance = value("appearance", AppearanceMode.system)
        lightThemeID = string("lightTheme") ?? Theme.defaultLight.id
        darkThemeID = string("darkTheme") ?? Theme.defaultDark.id
        customThemes = defaults.data(forKey: Self.prefix + "customThemes")
            .flatMap { try? JSONDecoder().decode([Theme].self, from: $0) } ?? []
        fontFamily = string("fontFamily") ?? ""
        let size = defaults.double(forKey: Self.prefix + "fontSize")
        fontSize = Self.fontSizes.contains(size) ? size : 13
        density = value("density", Density.regular)
        layout = value("layout", MessageLayout.columns)
        repeatNick = defaults.bool(forKey: Self.prefix + "repeatNick")
        let width = defaults.integer(forKey: Self.prefix + "nickColumnWidth")
        nickColumnWidth = Self.nickColumnWidths.contains(width) ? width : 12
        timestamps = value("timestamps", TimestampStyle.minutes)
        presence = value("presence", PresenceStyle.fold)
        nickColoring = value("nickColoring", NickColoring.rank)
        showFormatting = defaults.object(forKey: Self.prefix + "showFormatting") as? Bool ?? true
        highlightWords = defaults.stringArray(forKey: Self.prefix + "highlightWords") ?? []
        ignored = defaults.stringArray(forKey: Self.prefix + "ignored") ?? []
    }

    private nonisolated static func read(_ defaults: UserDefaults, _ key: String) -> String? {
        defaults.string(forKey: prefix + key)
    }

    private func set(_ value: Any?, _ key: String) {
        defaults.set(value, forKey: Self.prefix + key)
    }

    func ignore(_ nick: String) {
        if !ignored.contains(where: { $0.caseInsensitiveCompare(nick) == .orderedSame }) { ignored.append(nick) }
    }

    func unignore(_ nick: String) {
        ignored.removeAll { $0.caseInsensitiveCompare(nick) == .orderedSame }
    }

    // MARK: Themes

    var allThemes: [Theme] { Theme.builtIns + customThemes }

    func themes(dark: Bool) -> [Theme] { allThemes.filter { $0.isDark == dark } }

    /// The chosen theme for an appearance, falling back to the default if it
    /// was deleted or is for the other appearance.
    func theme(dark: Bool) -> Theme {
        let id = dark ? darkThemeID : lightThemeID
        return allThemes.first { $0.id == id && $0.isDark == dark } ?? (dark ? .defaultDark : .defaultLight)
    }

    /// Adds or replaces a custom theme.
    func save(_ theme: Theme) {
        guard !theme.isBuiltIn else { return }
        if let i = customThemes.firstIndex(where: { $0.id == theme.id }) {
            customThemes[i] = theme
        } else {
            customThemes.append(theme)
        }
    }

    func delete(_ theme: Theme) {
        customThemes.removeAll { $0.id == theme.id }
    }

    /// A custom copy of `theme` to edit.
    func copy(of theme: Theme) -> Theme {
        var copy = theme
        copy.id = "custom." + UUID().uuidString
        let names = Set(allThemes.map(\.name))
        var name = "\(theme.name) Copy"
        var n = 2
        while names.contains(name) {
            name = "\(theme.name) Copy \(n)"
            n += 1
        }
        copy.name = name
        return copy
    }

    /// Adds imported themes as new custom themes. Returns how many.
    @discardableResult
    func importThemes(from data: Data) throws -> Int {
        let themes = try Theme.decodeAll(data)
        for theme in themes {
            var theme = theme
            theme.id = "custom." + UUID().uuidString
            customThemes.append(theme)
        }
        return themes.count
    }

    // MARK: Appearance and fonts

    /// Applies the light/dark choice to every window.
    func applyAppearance() {
        NSApp?.appearance = switch appearance {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }

    func chatFont(scale: Double = 1) -> Font {
        let size = fontSize * scale
        switch fontFamily {
        case "": return .system(size: size)
        case Self.monospaced: return .system(size: size, design: .monospaced)
        default: return .custom(fontFamily, size: size)
        }
    }

    func nsFont(scale: Double = 1) -> NSFont {
        let size = fontSize * scale
        switch fontFamily {
        case "": return .systemFont(ofSize: size)
        case Self.monospaced: return .monospacedSystemFont(ofSize: size, weight: .regular)
        default:
            return NSFontManager.shared.font(withFamily: fontFamily, traits: [], weight: 5, size: size)
                ?? .systemFont(ofSize: size)
        }
    }
}
