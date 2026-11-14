import BackroomCore
import SwiftUI

/// A node of the pane tree: one pane, or a split of further nodes.
struct PaneTreeView: View {
    let node: PaneNode

    var body: some View {
        switch node {
        case .pane(let pane): PaneView(pane: pane)
        case .split(let split): SplitContainer(split: split)
        }
    }
}

/// Children side by side or stacked, with draggable dividers between them.
struct SplitContainer: View {
    @Environment(AppModel.self) private var model
    let split: Split
    /// Sizes while a divider is being dragged; saved when it's let go.
    @State private var dragging: [Double]?
    @State private var dragStart: [Double]?

    private var horizontal: Bool { split.axis == .horizontal }
    private static let divider: CGFloat = 1
    private var minimum: CGFloat { horizontal ? 220 : 120 }

    var body: some View {
        GeometryReader { geometry in
            let total = horizontal ? geometry.size.width : geometry.size.height
            let available = max(total - Self.divider * CGFloat(split.children.count - 1), 1)
            let fractions = dragging ?? split.fractions
            let layout = horizontal ? AnyLayout(HStackLayout(spacing: 0)) : AnyLayout(VStackLayout(spacing: 0))
            layout {
                ForEach(Array(split.children.enumerated()), id: \.element.id) { i, child in
                    if i > 0 { divider(after: i - 1, available: available) }
                    let size = available * (fractions.indices.contains(i) ? fractions[i] : 0)
                    PaneTreeView(node: child)
                        .frame(width: horizontal ? size : nil, height: horizontal ? nil : size)
                }
            }
        }
    }

    /// Between child `i` and `i + 1`: dragging moves space between them.
    private func divider(after i: Int, available: CGFloat) -> some View {
        Rectangle()
            .fill(Color(nsColor: .separatorColor))
            .frame(width: horizontal ? Self.divider : nil, height: horizontal ? nil : Self.divider)
            .overlay {
                Color.clear
                    .frame(width: horizontal ? 9 : nil, height: horizontal ? nil : 9)
                    .contentShape(Rectangle())
                    .onHover { inside in
                        let cursor: NSCursor = horizontal ? .resizeLeftRight : .resizeUpDown
                        if inside { cursor.push() } else { NSCursor.pop() }
                    }
                    .gesture(
                        DragGesture(minimumDistance: 1, coordinateSpace: .global)
                            .onChanged { drag in
                                let start = dragStart ?? split.fractions
                                dragStart = start
                                let delta = Double((horizontal ? drag.translation.width : drag.translation.height) / available)
                                let pair = start[i] + start[i + 1]
                                let low = Double(minimum / available)
                                guard pair > 2 * low else { return }
                                let first = min(max(start[i] + delta, low), pair - low)
                                var next = start
                                next[i] = first
                                next[i + 1] = pair - first
                                dragging = next
                            }
                            .onEnded { _ in
                                if let dragging { model.resizeSplit(split.id, fractions: dragging) }
                                dragging = nil
                                dragStart = nil
                            }
                    )
            }
            .zIndex(1)
    }
}

/// One pane: a header, then its buffer (or a hint while there's none).
struct PaneView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.theme) private var theme
    let pane: Pane

    var body: some View {
        let several = model.layout.panes.count > 1
        let focused = model.layout.focused == pane.id
        VStack(spacing: 0) {
            PaneHeader(pane: pane, focused: focused && several, closable: several)
            Divider()
            if let id = pane.buffer, model.buffers[id] != nil {
                BufferView(id: id, pane: pane.id)
                    .id("\(pane.id)|\(id.network)|\(id.key)")
            } else {
                ContentUnavailableView(
                    "No Conversation", systemImage: "bubble.left",
                    description: Text("Pick one in the sidebar."))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(theme.backgroundColor)
            }
        }
        .frame(minWidth: 0, maxWidth: .infinity, minHeight: 0, maxHeight: .infinity)
        .contentShape(Rectangle())
        .simultaneousGesture(TapGesture().onEnded { model.focus(pane.id) })
    }
}

/// The pane's name and topic (one line; click for all of it), and a close
/// button when there are several panes.
struct PaneHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(Preferences.self) private var prefs
    @Environment(\.theme) private var theme
    let pane: Pane
    let focused: Bool
    let closable: Bool
    @State private var expanded = false

    var body: some View {
        let buffer = pane.buffer.flatMap { model.buffers[$0] }
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: icon(buffer))
                .foregroundStyle(theme.mutedColor)
            Text(title(buffer))
                .fontWeight(.semibold)
                .foregroundStyle(theme.textColor)
                .lineLimit(1)
                .layoutPriority(1)
            if let topic = buffer?.topic {
                Text(MessageText.attributed(topic, formatting: prefs.showFormatting, theme: theme))
                    .foregroundStyle(theme.mutedColor)
                    .lineLimit(expanded ? nil : 1)
                    .help(stripFormatting(text: topic))
                    .textSelection(.enabled)
                    .onTapGesture { expanded.toggle() }
            }
            Spacer(minLength: 0)
            if closable {
                Button {
                    model.closePane(pane.id)
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .help("Close this pane (⌘W)")
            }
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(focused ? AnyShapeStyle(theme.accentColor.opacity(0.18)) : AnyShapeStyle(theme.surfaceColor))
    }

    private func icon(_ buffer: BufferModel?) -> String {
        switch buffer?.kind {
        case .channel: "number"
        case .query: "person"
        case .server: "network"
        case nil: "bubble.left"
        }
    }

    private func title(_ buffer: BufferModel?) -> String {
        guard let buffer else { return "No conversation" }
        guard buffer.kind == .server else { return buffer.name }
        return model.networkName(buffer.id.network)
    }
}
