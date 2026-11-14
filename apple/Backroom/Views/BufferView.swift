import AppKit
import BackroomCore
import SwiftUI

struct BufferView: View {
    @Environment(AppModel.self) private var model
    @Environment(Preferences.self) private var prefs
    @Environment(\.theme) private var theme
    let id: BufferID
    /// The pane showing this buffer.
    let pane: Pane.ID
    @State private var draft = ""
    @FocusState private var composerFocused: Bool
    @State private var completion: Completion?
    /// Position in the buffer's sent lines while stepping with ↑/↓.
    @State private var historyIndex: Int?

    var body: some View {
        let buffer = model.buffers[id]
        let style = ChatStyle(prefs: prefs, theme: theme)
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    HistoryHeader(buffer: buffer) { model.loadOlder(id) }
                    if let buffer {
                        ForEach(TimelineItem.build(
                            buffer.messages, presence: prefs.presence, unreadAfter: buffer.unreadLine,
                            isIgnored: buffer.mentions.isIgnored
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
                .onSubmit { submit(buffer) }
                .onKeyPress(.tab) {
                    complete(buffer)
                    return .handled
                }
                .onKeyPress(.upArrow) { stepHistory(buffer, back: true) ? .handled : .ignored }
                .onKeyPress(.downArrow) { stepHistory(buffer, back: false) ? .handled : .ignored }
                .onChange(of: draft) { _, new in
                    if let completion, new != completion.result { self.completion = nil }
                }
        }
        // #channel links (in messages and the topic) join the channel.
        .environment(\.openURL, OpenURLAction { url in
            guard let channel = MessageText.channel(from: url) else { return .systemAction }
            model.join(network: id.network, channel: channel)
            return .handled
        })
        // Typing goes to the focused pane; clicking into a composer focuses its pane.
        .onAppear { composerFocused = model.layout.focused == pane }
        .onChange(of: model.layout.focused) { _, focused in
            if focused == pane { composerFocused = true }
        }
        .onChange(of: composerFocused) { _, focused in
            if focused { model.focus(pane) }
        }
    }

    private func submit(_ buffer: BufferModel?) {
        let line = draft
        if model.send(line) {
            if let buffer, !line.trimmingCharacters(in: .whitespaces).isEmpty {
                buffer.sentHistory.append(line)
                if buffer.sentHistory.count > 200 { buffer.sentHistory.removeFirst() }
            }
            draft = ""
            historyIndex = nil
        }
        composerFocused = true
    }

    /// Tab: completes the last word with a nick, channel or command; again
    /// for the next match.
    private func complete(_ buffer: BufferModel?) {
        guard let buffer else { return }
        if var current = completion, draft == current.result {
            current.next()
            completion = current
            draft = current.result
            return
        }
        // Recent speakers first, then everyone else.
        var seen = Set<String>()
        let speakers = buffer.messages.reversed().filter { !$0.own && $0.kind.isChat }.map(\.sender)
        let nicks = (speakers + buffer.members.map(\.nick) + (buffer.kind == .query ? [buffer.name] : []))
            .filter { seen.insert($0.lowercased()).inserted }
        let channels = model.networks.first { $0.id == id.network }?.buffers
            .compactMap { model.buffers[$0] }.filter { $0.kind == .channel }.map(\.name) ?? []
        guard let started = Completion.start(draft, nicks: nicks, channels: channels) else { return }
        completion = started
        draft = started.result
    }

    /// ↑/↓ through lines sent here, when the composer is empty or showing
    /// one of them. Returns false to let the key move the cursor instead.
    private func stepHistory(_ buffer: BufferModel?, back: Bool) -> Bool {
        guard let history = buffer?.sentHistory, !history.isEmpty, !draft.contains("\n") else { return false }
        if let index = historyIndex, history.indices.contains(index), draft != history[index] { return false }
        if historyIndex == nil, !draft.isEmpty { return false }
        let next = back ? (historyIndex ?? history.count) - 1 : (historyIndex ?? history.count) + 1
        if next < 0 { return true }
        if next >= history.count {
            historyIndex = nil
            draft = ""
        } else {
            historyIndex = next
            draft = history[next]
        }
        return true
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
    let formatting: Bool
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
        formatting = prefs.showFormatting
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
            if buffer?.cleared == true {
                Text("Cleared")
            } else if buffer?.historyExhausted == true {
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
    @Environment(AppModel.self) private var model
    let message: ChatMessage
    let continued: Bool
    /// Read here, not by the list, so member changes only redraw visible rows.
    let buffer: BufferModel
    let style: ChatStyle

    private var theme: Theme { style.theme }

    var body: some View {
        let mention = buffer.isMention(message)
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
        .background {
            if mention {
                theme.accentColor.opacity(0.14)
                    .padding(.horizontal, -6)
                    .clipShape(RoundedRectangle(cornerRadius: 3))
            }
        }
    }

    private var prefix: String? { buffer.prefixByNick[message.sender.lowercased()] }

    private var nick: Text {
        Text(NickColor.label(message.sender, prefix: prefix)).bold()
            .foregroundColor(style.nickColor(message, prefix: prefix))
    }

    /// The message text with formatting and links.
    private var body_: Text {
        Text(MessageText.attributed(message.text, formatting: style.formatting, theme: theme))
    }

    private var plain: String { stripFormatting(text: message.text) }

    private var hidesNick: Bool { continued && !style.repeatNick }

    private var isEvent: Bool {
        switch message.kind {
        case .privmsg, .notice, .action, .reply, .error: false
        default: true
        }
    }

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
        case .join: Text("→").foregroundStyle(theme.mutedColor)
        case .part, .kick, .quit: Text("←").foregroundStyle(theme.mutedColor)
        case .reply: Text("–").foregroundStyle(theme.mutedColor)
        case .error: Text("!").bold().foregroundStyle(.red)
        case .action, .topic, .nick, .mode, .invite: Text("*").foregroundStyle(theme.mutedColor)
        }
    }

    @ViewBuilder private var columnText: some View {
        switch message.kind {
        case .privmsg, .notice, .reply: body_.foregroundStyle(theme.textColor)
        case .error: body_.foregroundStyle(.red)
        case .action: Text("\(nick) \(body_)").italic().foregroundStyle(theme.textColor)
        default: eventLine
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
                body_.foregroundStyle(theme.textColor)
                    .padding(.leading, style.nickWidth / 2)
            } else {
                Text("\(nick)  \(body_)").foregroundStyle(theme.textColor)
            }
        case .notice: Text("-\(nick)-  \(body_)").foregroundStyle(theme.textColor)
        case .action: Text("* \(nick) \(body_)").italic().foregroundStyle(theme.textColor)
        case .reply: body_.foregroundStyle(theme.textColor)
        case .error: Text("! \(body_)").foregroundStyle(.red)
        default: eventLine
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
                body_.foregroundStyle(theme.textColor)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            inline
        }
    }

    // MARK: Events: joins, parts, quits, kicks, topics, nicks, modes, invites

    @ViewBuilder private var eventLine: some View {
        if message.kind == .invite, buffer.kind == .server {
            // We were invited: offer to join.
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                event
                Button("Join \(plain)") { model.join(network: buffer.id.network, channel: plain) }
                    .buttonStyle(.link)
            }
        } else {
            event
        }
    }

    private var event: Text {
        let who = message.sender
        let line: String = switch message.kind {
        case .join: "\(who) joined"
        case .part: "\(who) left\(plain.isEmpty ? "" : " (\(plain))")"
        case .quit: "\(who) quit\(plain.isEmpty ? "" : " (\(plain))")"
        case .kick: "\(who) kicked \(plain)"
        case .topic: "\(who) set the topic: \(plain)"
        case .nick: message.own ? "You are now known as \(plain)" : "\(who) is now known as \(plain)"
        case .mode: "\(who) set mode \(plain)"
        case .invite: buffer.kind == .server ? "\(who) invited you to \(plain)" : "\(who) invited \(plain)"
        default: plain
        }
        let arrow = switch message.kind {
        case .join: "→ "
        case .part, .kick, .quit: "← "
        case .nick, .mode, .invite: "* "
        default: ""
        }
        return Text(style.layout == .columns ? line : arrow + line).foregroundColor(theme.mutedColor)
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
