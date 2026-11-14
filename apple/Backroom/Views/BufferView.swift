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
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    HistoryHeader(buffer: buffer) { model.loadOlder(id) }
                    ForEach(buffer?.messages ?? [], id: \.self) { message in
                        MessageRow(message: message)
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

    private func title(_ buffer: BufferState?) -> String {
        guard let buffer else { return "" }
        guard buffer.kind == .server else { return buffer.name }
        return model.networks.first { $0.id == id.network }?.network.name ?? "Server"
    }

    private func placeholder(_ buffer: BufferState?) -> String {
        guard let buffer else { return "" }
        return buffer.kind == .server ? "Commands, e.g. /join #channel" : "Message \(buffer.name)"
    }
}

/// Top of the message list: loads older history when it scrolls into view.
struct HistoryHeader: View {
    let buffer: BufferState?
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
        let sender = Text(message.sender).bold().foregroundColor(message.own ? .accentColor : .primary)
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
