import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Light/dark, a theme for each (picked from cards), custom themes, and the
/// chat font.
struct AppearanceSettings: View {
    @Environment(Preferences.self) private var prefs
    @Environment(\.colorScheme) private var colorScheme
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
            } footer: {
                Text("With System, Backroom follows macOS and switches between your light and dark theme.")
                    .foregroundStyle(.secondary)
            }

            themeSection(dark: false, selection: $prefs.lightThemeID)
            themeSection(dark: true, selection: $prefs.darkThemeID)

            Section("Import and Export") {
                LabeledContent("Import a Backroom theme file (JSON)") {
                    Button("Import…") { importing = true }
                }
                LabeledContent("Export your custom themes") {
                    Button("Export All…") { exporting = ThemeDocument(themes: prefs.customThemes) }
                        .disabled(prefs.customThemes.isEmpty)
                }
            }

            Section("Font") {
                FontMenu(selection: $prefs.fontFamily)
                LabeledContent("Size") {
                    HStack {
                        Slider(value: $prefs.fontSize, in: Preferences.fontSizes, step: 1)
                        Text("\(Int(prefs.fontSize)) pt").monospacedDigit().frame(width: 44, alignment: .trailing)
                    }
                }
                Picker("Line spacing", selection: $prefs.density) {
                    ForEach(Density.allCases) { Text($0.title).tag($0) }
                }
                ThemePreview(theme: prefs.theme(dark: colorScheme == .dark), font: prefs.chatFont())
                    .listRowInsets(EdgeInsets(top: 8, leading: 8, bottom: 8, trailing: 8))
            }
        }
        .formStyle(.grouped)
        .frame(minHeight: 620)
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

    /// "Light Mode" or "Dark Mode": a scrolling row of theme cards.
    private func themeSection(dark: Bool, selection: Binding<String>) -> some View {
        let selected = prefs.theme(dark: dark)
        let all = prefs.themes(dark: dark)
        let groups: [(title: String, themes: [Theme])] = [
            ("Default", all.filter { $0.id == (dark ? Theme.defaultDark.id : Theme.defaultLight.id) }),
            ("Built-in", all.filter { $0.isBuiltIn && !$0.id.hasPrefix("builtin.default") }),
            ("Custom", all.filter { !$0.isBuiltIn }),
        ].filter { !$0.themes.isEmpty }
        return Section {
            ScrollViewReader { proxy in
                ScrollView(.horizontal) {
                    HStack(alignment: .top, spacing: 14) {
                        ForEach(Array(groups.enumerated()), id: \.element.title) { index, group in
                            if index > 0 { Divider().frame(height: 96).padding(.top, 22) }
                            VStack(alignment: .leading, spacing: 6) {
                                Text(group.title).font(.callout).foregroundStyle(.secondary)
                                HStack(alignment: .top, spacing: 12) {
                                    ForEach(group.themes) { theme in
                                        ThemeCard(theme: theme, selected: theme.id == selected.id)
                                            .id(theme.id)
                                            .onTapGesture { selection.wrappedValue = theme.id }
                                            .contextMenu { menu(for: theme) }
                                    }
                                }
                            }
                        }
                    }
                    .padding(.horizontal, 4)
                    .padding(.vertical, 6)
                }
                .scrollIndicators(.automatic)
                .onAppear { proxy.scrollTo(selected.id, anchor: .center) }
            }
        } header: {
            HStack {
                Text(dark ? "Dark Mode" : "Light Mode")
                if (colorScheme == .dark) == dark {
                    Text("— Active").foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    editingTheme = prefs.copy(of: selected)
                } label: {
                    Label("New", systemImage: "plus.circle")
                }
                .buttonStyle(.borderless)
                .help("A new theme based on \(selected.name)")
            }
        }
    }

    @ViewBuilder private func menu(for theme: Theme) -> some View {
        if theme.isBuiltIn {
            Button("Copy and Edit…") { editingTheme = prefs.copy(of: theme) }
        } else {
            Button("Edit…") { editingTheme = theme }
            Button("Duplicate") { prefs.save(prefs.copy(of: theme)) }
            Button("Export…") { exporting = ThemeDocument(themes: [theme]) }
            Divider()
            Button("Delete", role: .destructive) { prefs.delete(theme) }
        }
    }
}

/// A theme as a card: "Aa" in its text colour, a channel chip and your
/// nick's colour, and its nick colours as overlapping dots.
struct ThemeCard: View {
    let theme: Theme
    let selected: Bool

    var body: some View {
        VStack(spacing: 6) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 6) {
                    Text("Aa")
                        .font(.system(size: 22))
                        .foregroundStyle(theme.textColor)
                    Spacer(minLength: 4)
                    Text("#")
                        .font(.system(size: 15, weight: .semibold, design: .monospaced))
                        .foregroundStyle(theme.textColor)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(theme.surfaceColor, in: RoundedRectangle(cornerRadius: 4))
                    Capsule()
                        .fill(theme.accentColor)
                        .frame(width: 9, height: 22)
                }
                HStack(spacing: -5) {
                    ForEach(Array(dots.enumerated()), id: \.offset) { _, color in
                        Circle()
                            .fill(color)
                            .frame(width: 15, height: 15)
                            .overlay(Circle().strokeBorder(theme.backgroundColor, lineWidth: 1.5))
                    }
                }
            }
            .padding(10)
            .frame(width: 132, height: 80)
            .background(theme.backgroundColor, in: RoundedRectangle(cornerRadius: 10))
            .overlay {
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(selected ? Color.accentColor : Color.secondary.opacity(0.3), lineWidth: selected ? 3 : 1)
            }
            // System colours (nil slots) resolve for the theme's own appearance.
            .environment(\.colorScheme, theme.isDark ? .dark : .light)
            Text(theme.name)
                .font(.callout)
                .fontWeight(selected ? .semibold : .regular)
                .foregroundStyle(selected ? .primary : .secondary)
                .lineLimit(1)
        }
        .frame(width: 132)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(theme.name)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }

    /// The surface colour, then the nick colours.
    private var dots: [Color] {
        [theme.surfaceColor] + Array(theme.nickPalette.prefix(7))
    }
}

/// Fonts that suit a chat client first: the system fonts, Nerd Fonts and
/// other monospaced fonts; everything else in a submenu.
struct FontMenu: View {
    @Binding var selection: String

    struct Families {
        var nerd: [String] = []
        var mono: [String] = []
        var other: [String] = []
    }

    /// Worked out once: checking every family takes a moment.
    @MainActor static let families: Families = {
        let manager = NSFontManager.shared
        var families = Families()
        for family in manager.availableFontFamilies where !family.hasPrefix(".") {
            if isNerdFont(family) {
                families.nerd.append(family)
            } else if manager.font(withFamily: family, traits: [], weight: 5, size: 12)?.isFixedPitch == true {
                families.mono.append(family)
            } else {
                families.other.append(family)
            }
        }
        return families
    }()

    /// "JetBrainsMono Nerd Font", "MesloLGS NF", "Hack NFM", …
    static func isNerdFont(_ family: String) -> Bool {
        family.contains("Nerd Font") || family.split(separator: " ").last.map { ["NF", "NFM", "NFP"].contains($0) } == true
    }

    var body: some View {
        let families = Self.families
        LabeledContent("Chat font") {
            Menu(title) {
                Button("System") { selection = "" }
                Button("System Monospaced (SF Mono)") { selection = Preferences.monospaced }
                if !families.nerd.isEmpty {
                    Section("Nerd Fonts") {
                        ForEach(families.nerd, id: \.self) { family in Button(family) { selection = family } }
                    }
                }
                if !families.mono.isEmpty {
                    Section("Monospaced") {
                        ForEach(families.mono, id: \.self) { family in Button(family) { selection = family } }
                    }
                }
                Divider()
                Menu("Other Fonts") {
                    ForEach(families.other, id: \.self) { family in Button(family) { selection = family } }
                }
            }
            .fixedSize()
        }
    }

    private var title: String {
        switch selection {
        case "": "System"
        case Preferences.monospaced: "System Monospaced"
        default: selection
        }
    }
}

/// A few sample lines in a theme and font.
struct ThemePreview: View {
    let theme: Theme
    var font: Font = .callout

    var body: some View {
        let lines: [(time: String, nick: String, prefix: Character?, text: String)] = [
            ("14:02", "alice", "@", "anyone tried the new build?"),
            ("14:03", "bob", "+", "yep, on Intel too  0O 1lI {}"),
            ("14:03", "carol", nil, "same here"),
        ]
        VStack(alignment: .leading, spacing: 3) {
            ForEach(lines, id: \.nick) { line in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(line.time).foregroundStyle(theme.mutedColor).monospacedDigit()
                    Text((line.prefix.map(String.init) ?? "") + line.nick).bold()
                        .foregroundStyle(theme.rankColor(line.prefix))
                        .frame(width: 70, alignment: .trailing)
                    Text(line.text).foregroundStyle(theme.textColor).lineLimit(1)
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("14:04").monospacedDigit()
                Text("→").frame(width: 70, alignment: .trailing)
                Text("dave joined")
            }
            .foregroundStyle(theme.mutedColor)
        }
        .font(font)
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.backgroundColor, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(theme.surfaceColor, lineWidth: 2))
        .environment(\.colorScheme, theme.isDark ? .dark : .light)
    }
}

/// Themes as a JSON file: one theme, or an array of them.
struct ThemeDocument: FileDocument {
    static let readableContentTypes: [UTType] = [.json]
    var themes: [Theme]

    init(themes: [Theme]) { self.themes = themes }

    init(configuration: ReadConfiguration) throws {
        themes = try Theme.decodeAll(configuration.file.regularFileContents ?? Data())
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = themes.count == 1 ? try encoder.encode(themes[0]) : try encoder.encode(themes)
        return FileWrapper(regularFileWithContents: data)
    }
}
