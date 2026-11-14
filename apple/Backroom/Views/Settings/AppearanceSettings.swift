import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Light/dark, a theme for each, custom themes, and the chat font.
struct AppearanceSettings: View {
    @Environment(Preferences.self) private var prefs
    @State private var editingTheme: Theme?
    @State private var importing = false
    @State private var exporting: ThemeDocument?
    @State private var importError: String?

    var body: some View {
        @Bindable var prefs = prefs
        Form {
            Section {
                Picker("Appearance", selection: $prefs.appearance) {
                    ForEach(AppearanceMode.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                themePicker("Light theme", dark: false, selection: $prefs.lightThemeID)
                themePicker("Dark theme", dark: true, selection: $prefs.darkThemeID)
            } footer: {
                Text("With System, Backroom follows macOS and switches between your light and dark theme.")
                    .foregroundStyle(.secondary)
            }

            Section("Preview") {
                HStack(spacing: 12) {
                    ThemePreview(theme: prefs.theme(dark: false))
                    ThemePreview(theme: prefs.theme(dark: true))
                }
                .listRowInsets(EdgeInsets(top: 8, leading: 8, bottom: 8, trailing: 8))
            }

            Section {
                if prefs.customThemes.isEmpty {
                    Text("Copy a theme to make your own.").foregroundStyle(.secondary)
                }
                ForEach(prefs.customThemes) { theme in
                    HStack {
                        ThemeSwatch(theme: theme)
                        Text(theme.name)
                        Text(theme.isDark ? "Dark" : "Light").foregroundStyle(.secondary)
                        Spacer()
                        Button("Edit…") { editingTheme = theme }
                        Menu {
                            Button("Duplicate") { prefs.save(prefs.copy(of: theme)) }
                            Button("Export…") { exporting = ThemeDocument(themes: [theme]) }
                            Divider()
                            Button("Delete", role: .destructive) { prefs.delete(theme) }
                        } label: {
                            Image(systemName: "ellipsis.circle")
                        }
                        .menuStyle(.borderlessButton)
                        .fixedSize()
                    }
                }
            } header: {
                Text("Custom Themes")
            } footer: {
                HStack {
                    Menu("New Theme From") {
                        ForEach(prefs.allThemes) { theme in
                            Button("\(theme.name) (\(theme.isDark ? "Dark" : "Light"))") {
                                editingTheme = prefs.copy(of: theme)
                            }
                        }
                    }
                    .fixedSize()
                    Spacer()
                    Button("Import…") { importing = true }
                    Button("Export All…") { exporting = ThemeDocument(themes: prefs.customThemes) }
                        .disabled(prefs.customThemes.isEmpty)
                }
            }

            Section("Font") {
                Picker("Chat font", selection: $prefs.fontFamily) {
                    Text("System").tag("")
                    Text("System Monospaced").tag(Preferences.monospaced)
                    Divider()
                    ForEach(NSFontManager.shared.availableFontFamilies, id: \.self) { family in
                        Text(family).tag(family)
                    }
                }
                LabeledContent("Size") {
                    HStack {
                        Slider(value: $prefs.fontSize, in: Preferences.fontSizes, step: 1)
                        Text("\(Int(prefs.fontSize)) pt").monospacedDigit().frame(width: 44, alignment: .trailing)
                    }
                }
                Picker("Line spacing", selection: $prefs.density) {
                    ForEach(Density.allCases) { Text($0.title).tag($0) }
                }
            }
        }
        .formStyle(.grouped)
        .frame(minHeight: 560)
        .sheet(item: $editingTheme) { theme in
            ThemeEditor(theme: theme)
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.json], allowsMultipleSelection: true) { result in
            do {
                for url in try result.get() {
                    let access = url.startAccessingSecurityScopedResource()
                    defer { if access { url.stopAccessingSecurityScopedResource() } }
                    try prefs.importThemes(from: Data(contentsOf: url))
                }
            } catch {
                importError = "Couldn't import the theme: \(error.localizedDescription)"
            }
        }
        .fileExporter(
            isPresented: Binding(get: { exporting != nil }, set: { if !$0 { exporting = nil } }),
            document: exporting, contentType: .json,
            defaultFilename: exporting?.themes.count == 1 ? exporting?.themes.first?.name : "Backroom Themes"
        ) { _ in }
        .alert(importError ?? "", isPresented: Binding(get: { importError != nil }, set: { if !$0 { importError = nil } })) {}
    }

    private func themePicker(_ title: String, dark: Bool, selection: Binding<String>) -> some View {
        Picker(title, selection: selection) {
            ForEach(prefs.themes(dark: dark)) { theme in
                Text(theme.name).tag(theme.id)
            }
        }
    }
}

/// A few sample lines in a theme.
struct ThemePreview: View {
    let theme: Theme

    var body: some View {
        let lines: [(time: String, nick: String, prefix: Character?, text: String)] = [
            ("14:02", "alice", "@", "anyone tried the new build?"),
            ("14:03", "bob", "+", "yep, on Intel too"),
            ("14:03", "carol", nil, "same here"),
        ]
        VStack(alignment: .leading, spacing: 3) {
            ForEach(lines, id: \.nick) { line in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(line.time).foregroundStyle(theme.mutedColor).font(.caption.monospacedDigit())
                    Text((line.prefix.map(String.init) ?? "") + line.nick).bold()
                        .foregroundStyle(theme.rankColor(line.prefix))
                        .frame(width: 50, alignment: .trailing)
                    Text(line.text).foregroundStyle(theme.textColor).lineLimit(1)
                }
            }
            HStack(spacing: 6) {
                Text("→ dave joined").foregroundStyle(theme.mutedColor)
                Spacer()
                Text("you").bold().foregroundStyle(theme.accentColor)
            }
            .font(.caption)
        }
        .font(.callout)
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.backgroundColor, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(theme.surfaceColor, lineWidth: 2))
        // System colours (nil slots) resolve for the theme's own appearance.
        .environment(\.colorScheme, theme.isDark ? .dark : .light)
    }
}

/// A small circle in the theme's background with its accent.
struct ThemeSwatch: View {
    let theme: Theme

    var body: some View {
        Circle()
            .fill(theme.backgroundColor)
            .overlay(Circle().inset(by: 4).fill(theme.accentColor))
            .overlay(Circle().strokeBorder(.separator))
            .frame(width: 16, height: 16)
            .environment(\.colorScheme, theme.isDark ? .dark : .light)
    }
}

/// Themes as a JSON file: one theme, or an array of them.
struct ThemeDocument: FileDocument {
    static let readableContentTypes: [UTType] = [.json]
    var themes: [Theme]

    init(themes: [Theme]) { self.themes = themes }

    init(configuration: ReadConfiguration) throws {
        let data = configuration.file.regularFileContents ?? Data()
        themes = try Theme.decodeAll(data)
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = themes.count == 1 ? try encoder.encode(themes[0]) : try encoder.encode(themes)
        return FileWrapper(regularFileWithContents: data)
    }
}
