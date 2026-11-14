import SwiftUI

/// How messages are laid out and what's shown.
struct MessageSettings: View {
    @Environment(Preferences.self) private var prefs

    var body: some View {
        @Bindable var prefs = prefs
        Form {
            Section {
                Picker("Layout", selection: $prefs.layout) {
                    ForEach(MessageLayout.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                LayoutSample(layout: prefs.layout, repeatNick: prefs.repeatNick)
                Toggle("Repeat nick on consecutive messages", isOn: $prefs.repeatNick)
                    .disabled(prefs.layout == .stacked)
                if prefs.layout == .columns {
                    Stepper(value: $prefs.nickColumnWidth, in: Preferences.nickColumnWidths) {
                        LabeledContent("Nick column", value: "\(prefs.nickColumnWidth) characters")
                    }
                }
            } header: {
                Text("Layout")
            } footer: {
                Text("Longer nicks are shortened from the front. A run of messages ends after 30 minutes of quiet.")
                    .foregroundStyle(.secondary)
            }
            Section("Show") {
                Picker("Timestamps", selection: $prefs.timestamps) {
                    ForEach(TimestampStyle.allCases) { Text($0.title).tag($0) }
                }
                Picker("Joins and parts", selection: $prefs.presence) {
                    ForEach(PresenceStyle.allCases) { Text($0.title).tag($0) }
                }
                Picker("Nick colours", selection: $prefs.nickColoring) {
                    ForEach(NickColoring.allCases) { Text($0.title).tag($0) }
                }
                Toggle("Let others see when I'm typing", isOn: $prefs.sendTyping)
                Toggle("mIRC colours and formatting", isOn: $prefs.showFormatting)
                    .help("Bold, italics and colours people put in their messages. Off shows plain text.")
            }
        }
        .formStyle(.grouped)
    }
}

/// What a layout looks like, in plain text.
struct LayoutSample: View {
    let layout: MessageLayout
    let repeatNick: Bool

    var body: some View {
        let second = repeatNick ? "alice" : ""
        let sample = switch layout {
        case .columns:
            """
            14:02    alice  anyone tried the new build?
            14:02    \(second.isEmpty ? "     " : second)  works fine here
            14:03      bob  yep, on Intel too
            """
        case .inline:
            """
            14:02 alice  anyone tried the new build?
            14:02 \(second.isEmpty ? "  " : second + "  ")works fine here
            14:03 bob  yep, on Intel too
            """
        case .stacked:
            """
            alice  14:02
            anyone tried the new build?
            works fine here
            bob  14:03
            yep, on Intel too
            """
        }
        Text(sample)
            .font(.system(.callout, design: .monospaced))
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
