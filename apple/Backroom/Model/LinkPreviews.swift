import AppKit
import BackroomCore
import Observation

/// Link previews, fetched by the core when the mouse rests on a link, and
/// kept for the session.
@MainActor
final class LinkPreviews {
    /// One link's preview. Observed on its own, so a preview arriving only
    /// redraws its card.
    @Observable
    @MainActor
    final class Entry {
        enum State {
            case loading
            case loaded(Preview)
            case failed
        }
        var state: State = .loading
    }

    struct Preview {
        let url: URL
        let kind: PreviewKind
        let title: String?
        let description: String?
        /// The site name, or the host.
        let site: String
        let image: NSImage?
    }

    private let previewer: (any LinkPreviewerProtocol)?
    private var entries: [String: Entry] = [:]
    /// Oldest first, for forgetting entries past `limit`.
    private var order: [String] = []
    private let limit: Int

    init(previewer: (any LinkPreviewerProtocol)?, limit: Int = 500) {
        self.previewer = previewer
        self.limit = limit
    }

    /// The preview for `url`, fetched on first request.
    func entry(for url: URL) -> Entry {
        let key = url.absoluteString
        if let entry = entries[key] { return entry }
        let entry = Entry()
        entries[key] = entry
        order.append(key)
        if order.count > limit { entries[order.removeFirst()] = nil }
        guard let previewer else {
            entry.state = .failed
            return entry
        }
        Task {
            guard let preview = await previewer.preview(url: key) else {
                entry.state = .failed
                return
            }
            entry.state = Self.preview(from: preview).map { .loaded($0) } ?? .failed
        }
        return entry
    }

    static func preview(from p: LinkPreview) -> Preview? {
        guard let url = URL(string: p.url) else { return nil }
        let image = p.image.flatMap { NSImage(data: $0) }
        // An image link whose image doesn't decode shows nothing.
        if p.kind == .image, image == nil { return nil }
        return Preview(
            url: url, kind: p.kind, title: p.title, description: p.description,
            site: p.siteName ?? url.host(percentEncoded: false) ?? url.absoluteString, image: image)
    }
}
