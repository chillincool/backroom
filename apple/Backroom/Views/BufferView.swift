import AppKit
import BackroomCore
import SwiftUI

struct BufferView: View {
    @Environment(AppModel.self) private var model
    @Environment(Preferences.self) private var prefs
    @Environment(\.theme) private var theme
    let id: BufferID
    @State private var draft = ""
    @FocusState private var composerFocused: Bool

    var body: some View {
        let buffer = model.buffers[id]
        let style = ChatStyle(prefs: prefs, theme: theme)
        VStack(spacing: 0) {
            if let buffer, let topic = buffer.topic, topic != buffer.dismissedTopic {
                TopicBanner(topic: topic) { buffer.dismissedTopic = topic }
                Divider()
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    HistoryHeader(buffer: buffer) { model.loadOlder(id) }
                    if let buffer {
                        ForEach(TimelineItem.build(
                            buffer.messages, presence: prefs.presence, unreadAfter: buffer.unreadLine
                        )) { item in
                            switch item {
                            case .message(let message, let continued):
                                MessageRow(message: message, continued: continued, buffer: buffer, style: style)
                            case .presence(let run):
                                PresenceRow(run: run, style: style)
                            case .day(let date):
                                DaySeparator(date: date, style: style)
                            case .unreadLine:
                                UnreadLine()
                            }
                        }
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .defaultScrollAnchor(.bottom)
            .background(theme.backgroundColor)

            Divider()
            TextField(placeholder(buffer), text: $draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(style.font)
                .foregroundStyle(theme.textColor)
                .lineLimit(1...5)
                .padding(10)
                .background(theme.backgroundColor)
                .focused($composerFocused)
                .onSubmit {
                    if model.send(draft) { draft = "" }
                    composerFocused = true
                }
        }
        .navigationTitle(title(buffer))
        .onAppear { composerFocused = true }
    }

    private func title(_ buffer: BufferModel?) -> String {
        guard let buffer else { return "" }
        guard buffer.kind == .server else { return buffer.name }
        return model.networks.first { $0.id == id.network }?.network.name ?? "Server"
    }

    private func placeholder(_ buffer: BufferModel?) -> String {
        guard let buffer else { return "" }
        return buffer.kind == .server ? "Commands, e.g. /join #channel" : "Message \(buffer.name)"
    }
}

/// Everything message rows need from the preferences and theme, worked out
/// once per redraw rather than per row.
struct ChatStyle {
    let theme: Theme
    let layout: MessageLayout
    let font: Font
    let timeFont: Font
    let timestamps: TimestampStyle
    let coloring: NickColoring
    let repeatNick: Bool
    let padding: CGFloat
    /// Column widths for the columns layout.
    let timeWidth: CGFloat
    let nickWidth: CGFloat

    @MainActor init(prefs: Preferences, theme: Theme) {
        self.theme = theme
        layout = prefs.layout
        font = prefs.chatFont()
        timeFont = prefs.chatFont(scale: 0.85).monospacedDigit()
        timestamps = prefs.timestamps
        coloring = prefs.nickColoring
        repeatNick = prefs.repeatNick
        padding = prefs.density.padding
        let nsFont = prefs.nsFont()
        nickWidth = Self.width(String(repeating: "n", count: prefs.nickColumnWidth), NSFontManager.shared.convert(nsFont, toHaveTrait: .boldFontMask))
        // The widest time this locale writes (12- or 24-hour).
        let samples = [1, 13].map { hour in
            Self.format(Calendar.current.date(bySettingHour: hour, minute: 59, second: 59, of: .now) ?? .now, prefs.timestamps)
        }
        let timeFont = prefs.nsFont(scale: 0.85)
        timeWidth = ceil(samples.map { Self.width($0, timeFont) }.max() ?? 0) + 2
    }

    private static func width(_ text: String, _ font: NSFont) -> CGFloat {
        ceil((text as NSString).size(withAttributes: [.font: font]).width)
    }

    static func format(_ date: Date, _ style: TimestampStyle) -> String {
        switch style {
        case .hidden: ""
        case .minutes: date.formatted(.dateTime.hour().minute())
        case .seconds: date.formatted(.dateTime.hour().minute().second())
        }
    }

    func nickColor(_ message: ChatMessage, prefix: String?) -> Color {
        message.own ? theme.accentColor : NickColor.color(nick: message.sender, prefix: prefix, theme: theme, coloring: coloring)
    }

    /// The timestamp, or nothing when hidden.
    @ViewBuilder func time(_ date: Date, fixedWidth: Bool) -> some View {
        if timestamps != .hidden {
            Text(Self.format(date, timestamps))
                .font(timeFont)
                .foregroundStyle(theme.mutedColor.opacity(0.8))
                .frame(width: fixedWidth ? timeWidth : nil, alignment: .trailing)
        }
    }
}

/// Top of the message list: loads older history when it scrolls into view.
struct HistoryHeader: View {
    @Environment(\.theme) private var theme
    let buffer: BufferModel?
    let loadOlder: () -> Void

    var body: some View {
        Group {
            if buffer?.historyExhausted == true {
                Text("Beginning of history")
            } else if buffer?.historyLoading == true {
                ProgressView().controlSize(.small)
            } else {
                Button("Load older messages", action: loadOlder).buttonStyle(.link)
                    .onAppear(perform: loadOlder)
            }
        }
        .font(.caption)
        .foregroundStyle(theme.mutedColor)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
    }
}

struct MessageRow: View {
    let message: ChatMessage
    let continued: Bool
    /// Read here, not by the list, so member changes only redraw visible rows.
    let buffer: BufferModel
    let style: ChatStyle

    private var theme: Theme { style.theme }

    var body: some View {
        Group {
            switch style.layout {
            case .columns: columns
            case .inline: inline
            case .stacked: stacked
            }
        }
        .font(style.font)
        .textSelection(.enabled)
        .padding(.vertical, style.padding)
    }

    private var prefix: String? { buffer.prefixByNick[message.sender.lowercased()] }

    private var nick: Text {
        Text(NickColor.label(message.sender, prefix: prefix)).bold()
            .foregroundColor(style.nickColor(message, prefix: prefix))
    }

    private var text: String { stripFormatting(text: message.text) }

    private var hidesNick: Bool { continued && !style.repeatNick }

    // MARK: Columns: time │ nick │ text

    private var columns: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            style.time(message.time, fixedWidth: true)
            nickCell
                .lineLimit(1)
                .truncationMode(.head)
                .frame(width: style.nickWidth, alignment: .trailing)
            columnText
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private var nickCell: some View {
        switch message.kind {
        case .privmsg: if hidesNick { Text("") } else { nick }
        case .notice: Text("-\(nick)-")
        case .action, .topic: Text("*").foregroundStyle(theme.mutedColor)
        case .join: Text("→").foregroundStyle(theme.mutedColor)
        case .part, .kick: Text("←").foregroundStyle(theme.mutedColor)
        }
    }

    @ViewBuilder private var columnText: some View {
        switch message.kind {
        case .privmsg, .notice: Text(text).foregroundStyle(theme.textColor)
        case .action: Text("\(nick) \(text)").italic().foregroundStyle(theme.textColor)
        default: event
        }
    }

    // MARK: Inline: time nick text

    private var inline: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            style.time(message.time, fixedWidth: true)
            inlineText.frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private var inlineText: some View {
        switch message.kind {
        case .privmsg:
            if hidesNick {
                Text(text).foregroundStyle(theme.textColor)
                    .padding(.leading, style.nickWidth / 2)
            } else {
                Text("\(nick)  \(text)").foregroundStyle(theme.textColor)
            }
        case .notice: Text("-\(nick)-  \(text)").foregroundStyle(theme.textColor)
        case .action: Text("* \(nick) \(text)").italic().foregroundStyle(theme.textColor)
        default: event
        }
    }

    // MARK: Stacked: nick and time above the run

    @ViewBuilder private var stacked: some View {
        if message.kind == .privmsg {
            VStack(alignment: .leading, spacing: 1) {
                if !continued {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        nick
                        style.time(message.time, fixedWidth: false)
                    }
                    .padding(.top, 6)
                }
                Text(text).foregroundStyle(theme.textColor)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            inline
        }
    }

    // MARK: Joins, parts, kicks and topics

    private var event: Text {
        let line: String = switch message.kind {
        case .join: "\(message.sender) joined"
        case .part: "\(message.sender) left\(text.isEmpty ? "" : " (\(text))")"
        case .kick: "\(message.sender) kicked \(text)"
        case .topic: "\(message.sender) set the topic: \(text)"
        default: text
        }
        let arrow = switch message.kind {
        case .join: "→ "
        case .part, .kick: "← "
        default: ""
        }
        return Text(style.layout == .columns ? line : arrow + line).foregroundColor(theme.mutedColor)
    }
}

/// The channel topic, closable; it comes back when the topic changes.
struct TopicBanner: View {
    @Environment(\.theme) private var theme
    let topic: String
    let close: () -> Void

    var body: some View {
        let text = stripFormatting(text: topic)
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "text.quote").foregroundStyle(theme.mutedColor)
            Text(text)
                .foregroundStyle(theme.textColor)
                .lineLimit(2)
                .textSelection(.enabled)
                .help(text)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: close) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("Hide the topic until it changes")
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(theme.surfaceColor)
    }
}

/// A run of joins and parts on one line.
struct PresenceRow: View {
    let run: [ChatMessage]
    let style: ChatStyle

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            style.time(run.last?.time ?? .now, fixedWidth: true)
            if style.layout == .columns {
                Text("⇄")
                    .frame(width: style.nickWidth, alignment: .trailing)
            }
            Text(TimelineItem.summary(run))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(style.font)
        .foregroundStyle(style.theme.mutedColor)
        .textSelection(.enabled)
        .padding(.vertical, style.padding)
    }
}

/// Above the first message that arrived since you last looked.
struct UnreadLine: View {
    var body: some View {
        HStack(spacing: 6) {
            Rectangle().fill(Color.red).frame(height: 1)
            Text("New")
                .font(.caption2.bold())
                .foregroundStyle(.white)
                .padding(.horizontal, 6)
                .padding(.vertical, 1)
                .background(Capsule().fill(Color.red))
        }
        .padding(.vertical, 4)
        .accessibilityLabel("New messages below")
    }
}

/// Where the day changes in the timeline.
struct DaySeparator: View {
    let date: Date
    let style: ChatStyle

    var body: some View {
        HStack(spacing: 8) {
            line
            Text(date, format: .dateTime.weekday(.wide).day().month(.wide).year())
                .font(.caption.weight(.medium))
                .foregroundStyle(style.theme.mutedColor)
                .fixedSize()
            line
        }
        .padding(.vertical, 8)
    }

    private var line: some View {
        Rectangle().fill(style.theme.mutedColor.opacity(0.3)).frame(height: 1)
    }
}
