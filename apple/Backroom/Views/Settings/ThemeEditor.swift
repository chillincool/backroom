import SwiftUI

/// Edits a custom theme (or a new copy of any theme) and saves it on Done.
struct ThemeEditor: View {
    @Environment(Preferences.self) private var prefs
    @Environment(\.dismiss) private var dismiss
    @State private var theme: Theme

    init(theme: Theme) {
        _theme = State(initialValue: theme)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $theme.name)
                    Picker("For", selection: $theme.isDark) {
                        Text("Light appearance").tag(false)
                        Text("Dark appearance").tag(true)
                    }
                }
                Section("Preview") {
                    ThemePreview(theme: theme)
                }
                Section {
                    slot("Background", \.background, fallback: theme.backgroundColor)
                    slot("Banner and separators", \.surface, fallback: theme.surfaceColor)
                    slot("Text", \.text, fallback: theme.textColor)
                    slot("Muted text", \.muted, fallback: theme.mutedColor)
                    slot("Your nick", \.accent, fallback: theme.accentColor)
                } header: {
                    Text("Colours")
                } footer: {
                    Text("↺ goes back to the system colour, which follows the macOS accent and appearance.")
                        .foregroundStyle(.secondary)
                }
                Section("Nicks by rank") {
                    slot("Owners ~", \.ranks.owner, fallback: theme.rankColor("~"))
                    slot("Admins &", \.ranks.admin, fallback: theme.rankColor("&"))
                    slot("Operators @", \.ranks.op, fallback: theme.rankColor("@"))
                    slot("Half-operators %", \.ranks.halfop, fallback: theme.rankColor("%"))
                    slot("Voiced +", \.ranks.voice, fallback: theme.rankColor("+"))
                }
                Section {
                    LabeledContent("Colours") {
                        HStack(spacing: 4) {
                            ForEach(theme.nicks.indices, id: \.self) { i in
                                ColorPicker("", selection: nickBinding(i), supportsOpacity: false)
                                    .labelsHidden()
                            }
                            Button {
                                if theme.nicks.isEmpty { theme.nicks = systemPalette }
                                theme.nicks.append(theme.nicks.last ?? "#808080")
                            } label: { Image(systemName: "plus") }
                            Button {
                                theme.nicks.removeLast()
                            } label: { Image(systemName: "minus") }
                                .disabled(theme.nicks.isEmpty)
                        }
                        .buttonStyle(.borderless)
                    }
                    if theme.nicks.isEmpty {
                        Button("Customise the System Palette") { theme.nicks = systemPalette }
                    }
                } header: {
                    Text("Nicks by name")
                } footer: {
                    Text("Used when Settings › Messages › Nick colours is By nick.")
                        .foregroundStyle(.secondary)
                }
            }
            .formStyle(.grouped)
            .frame(width: 460, height: 640)
            .navigationTitle(theme.name)
            .environment(\.colorScheme, theme.isDark ? .dark : .light)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        prefs.save(theme)
                        dismiss()
                    }
                    .disabled(theme.name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
    }

    private var systemPalette: [String] {
        Theme.systemNickPalette.map { $0.hex(dark: theme.isDark) }
    }

    private func slot(_ title: String, _ path: WritableKeyPath<Theme, String?>, fallback: Color) -> some View {
        LabeledContent(title) {
            HStack(spacing: 6) {
                if theme[keyPath: path] != nil {
                    Button {
                        theme[keyPath: path] = nil
                    } label: { Image(systemName: "arrow.uturn.backward") }
                        .buttonStyle(.borderless)
                        .help("Use the system colour")
                } else {
                    Text("System").font(.caption).foregroundStyle(.secondary)
                }
                ColorPicker("", selection: Binding(
                    get: { Color(hex: theme[keyPath: path]) ?? fallback },
                    set: { theme[keyPath: path] = $0.hex(dark: theme.isDark) }
                ), supportsOpacity: false)
                .labelsHidden()
            }
        }
    }

    private func nickBinding(_ i: Int) -> Binding<Color> {
        Binding(
            get: { theme.nicks.indices.contains(i) ? Color(hex: theme.nicks[i]) ?? .gray : .gray },
            set: { if theme.nicks.indices.contains(i) { theme.nicks[i] = $0.hex(dark: theme.isDark) } })
    }
}
