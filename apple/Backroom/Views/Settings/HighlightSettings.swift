import SwiftUI

/// Words that count as mentions, and people to ignore.
struct HighlightSettings: View {
    @Environment(Preferences.self) private var prefs

    var body: some View {
        @Bindable var prefs = prefs
        Form {
            Section {
                WordList(items: $prefs.highlightWords, placeholder: "Add a word or name", empty: "Only your nick")
            } header: {
                Text("Highlight Words")
            } footer: {
                Text("Messages with your nick or one of these words (whole words, any case) are highlighted and counted in red.")
                    .foregroundStyle(.secondary)
            }
            Section {
                WordList(items: $prefs.ignored, placeholder: "Add a nick", empty: "Nobody")
            } header: {
                Text("Ignored")
            } footer: {
                Text("Their messages, joins and parts are hidden. /ignore and /unignore work too.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

/// An editable list of words: one per row, removable, with a field to add.
struct WordList: View {
    @Binding var items: [String]
    let placeholder: String
    let empty: String
    @State private var new = ""

    var body: some View {
        if items.isEmpty {
            Text(empty).foregroundStyle(.secondary)
        }
        ForEach(items, id: \.self) { item in
            HStack {
                Text(item)
                Spacer()
                Button {
                    items.removeAll { $0 == item }
                } label: { Image(systemName: "minus.circle.fill").foregroundStyle(.secondary) }
                    .buttonStyle(.borderless)
                    .help("Remove \(item)")
            }
        }
        HStack {
            TextField(placeholder, text: $new)
                .onSubmit(add)
            Button("Add", action: add)
                .disabled(trimmed.isEmpty)
        }
    }

    private var trimmed: String { new.trimmingCharacters(in: .whitespaces) }

    private func add() {
        guard !trimmed.isEmpty else { return }
        if !items.contains(where: { $0.caseInsensitiveCompare(trimmed) == .orderedSame }) {
            items.append(trimmed)
        }
        new = ""
    }
}
