import BackroomCore
import SwiftUI

/// Searches history on soju: this conversation, or every one on the network.
struct SearchView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    @State private var text = ""
    @State private var everywhere = false
    @State private var selection: String?
    @FocusState private var fieldFocused: Bool

    var body: some View {
        let state = model.search?.network == network ? model.search : nil
        let current = model.selectedBuffer.flatMap { $0.id.network == network && $0.kind != .server ? $0 : nil }
        VStack(spacing: 0) {
            HStack {
                TextField("Search messages", text: $text)
                    .textFieldStyle(.roundedBorder)
                    .focused($fieldFocused)
                    .onSubmit { run(current) }
                Picker("In", selection: $everywhere) {
                    Text(current?.name ?? "This conversation").tag(false)
                    Text("All of \(model.networkName(network))").tag(true)
                }
                .labelsHidden()
                .fixedSize()
                .disabled(current == nil)
                Button("Search") { run(current) }
                    .disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(10)
            Divider()
            List(selection: $selection) {
                if let state {
                    ForEach(state.hits, id: \.message.stableID) { hit in
                        SearchHitRow(hit: hit, showBuffer: everywhere || current == nil)
                            .tag(hit.message.stableID)
                    }
                }
            }
            .contextMenu(forSelectionType: String.self) { _ in } primaryAction: { ids in
                if let id = ids.first { open(id, in: state) }
            }
            .overlay {
                if let state, state.hits.isEmpty, state.pending.isEmpty {
                    ContentUnavailableView.search(text: state.text)
                } else if state == nil {
                    ContentUnavailableView(
                        "Search History", systemImage: "magnifyingglass",
                        description: Text("Finds messages in the stored history. Double-click a result to go to it."))
                }
            }
            HStack {
                if let state, !state.pending.isEmpty {
                    ProgressView().controlSize(.small)
                    Text("Searching…")
                } else if let state {
                    Text("\(state.hits.count) results" + (state.hits.count >= 100 ? " (the newest 100 per conversation)" : ""))
                }
                if let error = state?.errors.first {
                    Text(error).foregroundStyle(.red).lineLimit(1).help(state?.errors.joined(separator: "\n") ?? "")
                }
                Spacer()
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(8)
        }
        .frame(minWidth: 620, minHeight: 420)
        .navigationTitle("Search \(model.networkName(network))")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Go to Message") { if let selection { open(selection, in: state) } }
                    .disabled(selection == nil)
            }
        }
        .onAppear {
            fieldFocused = true
            if current == nil { everywhere = true }
            if let state { text = state.text }
        }
    }

    private func run(_ current: BufferModel?) {
        model.startSearch(network: network, in: everywhere ? nil : current?.name, text: text)
    }

    private func open(_ id: String, in state: AppModel.SearchState?) {
        guard let hit = state?.hits.first(where: { $0.message.stableID == id }) else { return }
        model.reveal(network: network, buffer: hit.buffer, message: hit.message)
        dismiss()
    }
}

struct SearchHitRow: View {
    let hit: SearchHit
    let showBuffer: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                if showBuffer { Text(hit.buffer).fontWeight(.semibold) }
                Text(hit.message.sender).foregroundStyle(.secondary)
                Spacer()
                Text(hit.message.time, format: .dateTime.year().month().day().hour().minute())
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Text(stripFormatting(text: hit.message.text))
                .lineLimit(3)
        }
        .padding(.vertical, 2)
    }
}
