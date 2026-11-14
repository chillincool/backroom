import Foundation
import SwiftUI
import Testing

@testable import Backroom

@MainActor @Suite struct PreferencesTests {
    func defaults() -> UserDefaults { UserDefaults(suiteName: "backroom-prefs-\(UUID().uuidString)")! }

    @Test func defaultsAreColumnsWithoutRepeatedNicks() {
        let prefs = Preferences(defaults: defaults())
        #expect(prefs.layout == .columns)
        #expect(!prefs.repeatNick)
        #expect(prefs.appearance == .system)
        #expect(prefs.theme(dark: false) == .defaultLight)
        #expect(prefs.theme(dark: true) == .defaultDark)
    }

    @Test func choicesPersist() {
        let store = defaults()
        let prefs = Preferences(defaults: store)
        prefs.layout = .stacked
        prefs.repeatNick = true
        prefs.darkThemeID = Theme.dracula.id
        prefs.fontSize = 16
        let again = Preferences(defaults: store)
        #expect(again.layout == .stacked)
        #expect(again.repeatNick)
        #expect(again.theme(dark: true) == .dracula)
        #expect(again.fontSize == 16)
    }

    @Test func themesFallBackToTheDefault() {
        let prefs = Preferences(defaults: defaults())
        // A dark theme chosen as the light one, and a deleted theme.
        prefs.lightThemeID = Theme.dracula.id
        prefs.darkThemeID = "custom.gone"
        #expect(prefs.theme(dark: false) == .defaultLight)
        #expect(prefs.theme(dark: true) == .defaultDark)
    }

    @Test func customThemesCopySaveAndDelete() {
        let store = defaults()
        let prefs = Preferences(defaults: store)
        var copy = prefs.copy(of: .nord)
        #expect(copy.name == "Nord Copy")
        #expect(!copy.isBuiltIn)
        copy.background = "#000000"
        prefs.save(copy)
        #expect(prefs.copy(of: .nord).name == "Nord Copy 2")
        prefs.darkThemeID = copy.id
        #expect(Preferences(defaults: store).theme(dark: true).background == "#000000")

        prefs.delete(copy)
        #expect(prefs.customThemes.isEmpty)
        #expect(prefs.theme(dark: true) == .defaultDark)
        // Built-ins can't be overwritten.
        prefs.save(.nord)
        #expect(prefs.customThemes.isEmpty)
    }

    @Test func importGivesNewIDs() throws {
        let prefs = Preferences(defaults: defaults())
        let one = try JSONEncoder().encode(Theme.solarizedDark)
        #expect(try prefs.importThemes(from: one) == 1)
        let many = try JSONEncoder().encode([Theme.gruvboxLight, Theme.gruvboxDark])
        #expect(try prefs.importThemes(from: many) == 2)
        #expect(prefs.customThemes.count == 3)
        #expect(prefs.customThemes.allSatisfy { !$0.isBuiltIn })
        #expect(Set(prefs.customThemes.map(\.id)).count == 3)
        #expect(throws: (any Error).self) { try prefs.importThemes(from: Data("nope".utf8)) }
    }

    @Test func hexColoursRoundTrip() {
        #expect(Color(hex: "#268bd2")?.hex(dark: false) == "#268bd2")
        #expect(Color(hex: "fdf6e3")?.hex(dark: true) == "#fdf6e3")
        #expect(Color(hex: "#12345") == nil)
        #expect(Color(hex: nil) == nil)
    }

    @Test func builtInThemesAreWellFormed() {
        #expect(Set(Theme.builtIns.map(\.id)).count == Theme.builtIns.count)
        for theme in Theme.builtIns {
            #expect(theme.isBuiltIn)
            for hex in [theme.background, theme.surface, theme.text, theme.muted, theme.accent].compactMap({ $0 }) + theme.nicks {
                #expect(Color(hex: hex) != nil, "\(theme.id): \(hex)")
            }
        }
    }

    @Test func nickColoursAreStable() {
        #expect(NickColor.paletteIndex("Alice", count: 8) == NickColor.paletteIndex("alice", count: 8))
        #expect((0..<8).contains(NickColor.paletteIndex("bob", count: 8)))
    }

    @Test func nerdFontsAreRecognised() {
        #expect(FontMenu.isNerdFont("JetBrainsMono Nerd Font"))
        #expect(FontMenu.isNerdFont("MesloLGS NF"))
        #expect(FontMenu.isNerdFont("Hack NFM"))
        #expect(!FontMenu.isNerdFont("Menlo"))
        #expect(!FontMenu.isNerdFont("NFL Sans"))
    }
}
