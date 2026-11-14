import SwiftUI

/// The tabs, in the title bar; only there while there are several.
struct TabStrip: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 4) {
                ForEach(model.tabs) { tab in
                    TabChip(tab: tab, current: tab.id == model.currentTab)
                }
            }
            .padding(.horizontal, 4)
        }
        .scrollIndicators(.never)
        .frame(minWidth: 200, idealWidth: CGFloat(model.tabs.count) * 160, maxWidth: 900)
    }
}

struct TabChip: View {
    @Environment(AppModel.self) private var model
    @Environment(\.theme) private var theme
    let tab: WorkspaceTab
    let current: Bool
    @State private var hovering = false
    @State private var editing = false
    @State private var draft = ""
    @FocusState private var fieldFocused: Bool

    var body: some View {
        let title = model.tabTitle(tab)
        let unread = current ? (0, 0) : model.tabUnread(tab)
        HStack(spacing: 6) {
            if editing {
                TextField("Tab name", text: $draft, prompt: Text(title))
                    .textFieldStyle(.plain)
                    .focused($fieldFocused)
                    .onSubmit(commit)
                    .onExitCommand { editing = false }
                    .onChange(of: fieldFocused) { _, focused in if !focused { commit() } }
            } else {
                Text(title)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .fontWeight(current ? .semibold : .regular)
                    .help(title)
            }
            if unread.0 > 0 {
                // A dot for news, red with a count when it's for you.
                if unread.1 > 0 {
                    UnreadBadge(count: unread.1, mentions: unread.1)
                } else {
                    Circle().fill(.tint).frame(width: 6, height: 6)
                }
            }
            Button {
                model.closeTab(tab.id)
            } label: {
                Image(systemName: "xmark").font(.caption2.bold())
            }
            .buttonStyle(.borderless)
            .opacity(hovering || current ? 1 : 0)
            .help("Close Tab")
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .frame(minWidth: 80, maxWidth: 200)
        .background(
            current ? AnyShapeStyle(theme.accentColor.opacity(0.22))
                : AnyShapeStyle(Color.primary.opacity(hovering ? 0.08 : 0.03)),
            in: Capsule())
        .contentShape(Capsule())
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { startEditing(title) }
        .onTapGesture { model.showTab(tab.id) }
        .contextMenu {
            Button("Rename…") { startEditing(title) }
            Button("Close Tab") { model.closeTab(tab.id) }
        }
        .draggable(tab.id.uuidString)
        .dropDestination(for: String.self) { items, _ in
            guard let id = items.first.flatMap(UUID.init(uuidString:)) else { return false }
            model.moveTab(id, to: tab.id)
            return true
        }
    }

    private func startEditing(_ title: String) {
        draft = tab.name ?? ""
        editing = true
        fieldFocused = true
    }

    private func commit() {
        guard editing else { return }
        model.renameTab(tab.id, to: draft)
        editing = false
    }
}
