import AppKit
import BackroomCore
import Foundation
import Testing

@testable import Backroom

/// Answers from a table and counts requests.
final class FakePreviewer: LinkPreviewerProtocol, @unchecked Sendable {
    let answers: [String: LinkPreview]
    private let lock = NSLock()
    private var _requests: [String] = []
    var requests: [String] { lock.withLock { _requests } }

    init(_ answers: [String: LinkPreview]) { self.answers = answers }

    func preview(url: String) async -> LinkPreview? {
        lock.withLock { _requests.append(url) }
        return answers[url]
    }
}

/// A 2×1 PNG.
private let png: Data = {
    let image = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 1, bitsPerSample: 8, samplesPerPixel: 4,
        hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    return image.representation(using: .png, properties: [:])!
}()

@MainActor
private func settle(_ entry: LinkPreviews.Entry) async {
    for _ in 0..<100 {
        if case .loading = entry.state { await Task.yield() } else { return }
    }
}

@MainActor
@Suite struct LinkPreviewTests {
    @Test func loadsAPageOnceAndReusesIt() async throws {
        let page = LinkPreview(
            url: "https://example.com/final", kind: .page, title: "Title", description: "Desc",
            siteName: nil, image: png)
        let fake = FakePreviewer(["https://example.com/a": page])
        let previews = LinkPreviews(previewer: fake)
        let url = try #require(URL(string: "https://example.com/a"))

        let entry = previews.entry(for: url)
        await settle(entry)
        guard case .loaded(let preview) = entry.state else {
            Issue.record("not loaded: \(String(describing: entry.state))")
            return
        }
        #expect(preview.title == "Title")
        #expect(preview.site == "example.com")
        #expect(preview.image?.size == NSSize(width: 2, height: 1))
        #expect(previews.entry(for: url) === entry)
        #expect(fake.requests == ["https://example.com/a"])
    }

    @Test func nothingToShowFails() async throws {
        let broken = LinkPreview(
            url: "https://example.com/x.png", kind: .image, title: nil, description: nil, siteName: nil,
            image: Data([1, 2, 3]))
        let previews = LinkPreviews(previewer: FakePreviewer(["https://example.com/x.png": broken]))
        for link in ["https://example.com/missing", "https://example.com/x.png"] {
            let entry = previews.entry(for: try #require(URL(string: link)))
            await settle(entry)
            guard case .failed = entry.state else {
                Issue.record("\(link): \(String(describing: entry.state))")
                continue
            }
        }
    }

    @Test func forgetsTheOldestPastTheLimit() async throws {
        let fake = FakePreviewer([:])
        let previews = LinkPreviews(previewer: fake, limit: 2)
        let urls = try (1...3).map { try #require(URL(string: "https://example.com/\($0)")) }
        let first = previews.entry(for: urls[0])
        _ = previews.entry(for: urls[1])
        _ = previews.entry(for: urls[2])
        #expect(previews.entry(for: urls[0]) !== first)
    }

    @Test func firstWebLink() {
        #expect(MessageText.firstWebLink("see #chan and https://a.example/x, also http://b.example")?.absoluteString
            == "https://a.example/x")
        #expect(MessageText.firstWebLink("mail me@example.com or #chan") == nil)
        // Formatting codes don't end up in the link.
        #expect(MessageText.firstWebLink("\u{2}https://a.example/bold\u{2} done")?.absoluteString
            == "https://a.example/bold")
    }

    @Test func linksAreMarkedButLookLikeText() throws {
        let text = MessageText.nsAttributed(
            "go to https://a.example now or #rust", formatting: true, theme: .defaultLight,
            font: .systemFont(ofSize: 13), color: .labelColor)
        let string = text.string as NSString
        let web = string.range(of: "https://a.example")
        #expect(text.attribute(.chatLink, at: web.location, effectiveRange: nil) as? URL
            == URL(string: "https://a.example"))
        #expect(text.attribute(.underlineStyle, at: web.location, effectiveRange: nil) == nil)
        #expect(text.attribute(.foregroundColor, at: web.location, effectiveRange: nil) as? NSColor == .labelColor)
        #expect(text.attribute(.link, at: web.location, effectiveRange: nil) == nil)
        let channel = string.range(of: "#rust")
        let url = try #require(text.attribute(.chatLink, at: channel.location, effectiveRange: nil) as? URL)
        #expect(MessageText.channel(from: url) == "#rust")
        #expect(text.attribute(.chatLink, at: 0, effectiveRange: nil) == nil)
    }
}
