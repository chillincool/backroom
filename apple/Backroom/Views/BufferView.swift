import BackroomCore
import SwiftUI

struct BufferView: View {
    @Environment(AppModel.self) private var model
    let id: BufferID
    @State private var draft = ""
    @FocusState private var composerFocused: Bool

    var body: some View {
        let buffer = model.buffers[id]
        VStack(spacing: 0) {
            if let buffer, let topic = buffer.topic, topic != buffer.dismissedTopic {
                TopicBanner(topic: topic) { buffer.dismissedTopic = topic }
                Divider()
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    HistoryHeader(buffer: buffer) { model.loadOlder(id) }
                    if let buffer {
                        ForEach(TimelineItem.build(buffer.messages)) { item in
                            switch item {
                            case .message(let message):
                                MessageRow(message: message, buffer: buffer)
                            case .presence(let run):
                                PresenceRow(run: run)
                            }
                        }
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .defaultScrollAnchor(.bottom)

            Divider()
            TextField(placeholder(buffer), text: $draft, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...5)
                .padding(10)
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

/// Top of the message list: loads older history when it scrolls into view.
struct HistoryHeader: View {
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
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
    }
}

struct MessageRow: View {
    let message: ChatMessage
    /// Read here, not by the list, so member changes only redraw visible rows.
    let buffer: BufferModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(message.time, format: .dateTime.hour().minute())
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
            line
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 1)
    }

    @ViewBuilder private var line: some View {
        let text = stripFormatting(text: message.text)
        let prefix = buffer.prefixByNick[message.sender.lowercased()]
        let color = message.own ? Color.accentColor : NickColor.color(prefix: prefix)
        let sender = Text(NickColor.label(message.sender, prefix: prefix)).bold().foregroundColor(color)
        switch message.kind {
        case .privmsg:
            Text("\(sender)  \(text)")
        case .notice:
            Text("-\(sender)-  \(text)").foregroundStyle(.secondary)
        case .action:
            Text("* \(sender) \(text)").italic()
        case .join:
            Text("→ \(message.sender) joined").foregroundStyle(.secondary)
        case .part:
            Text("← \(message.sender) left\(text.isEmpty ? "" : " (\(text))")").foregroundStyle(.secondary)
        case .kick:
            Text("← \(message.sender) kicked: \(text)").foregroundStyle(.secondary)
        case .topic:
            Text("\(message.sender) set the topic: \(text)").foregroundStyle(.secondary)
        }
    }
}

/// The channel topic, closable; it comes back when the topic changes.
struct TopicBanner: View {
    let topic: String
    let close: () -> Void

    var body: some View {
        let text = stripFormatting(text: topic)
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "text.quote").foregroundStyle(.secondary)
            Text(text)
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
        .background(.bar)
    }
}

/// A run of joins and parts on one line.
struct PresenceRow: View {
    let run: [ChatMessage]

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(run.last?.time ?? .now, format: .dateTime.hour().minute())
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
            Text(TimelineItem.summary(run))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 1)
    }
}
