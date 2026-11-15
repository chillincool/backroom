import AppKit
import SwiftUI

extension NSAttributedString.Key {
    /// A link in message text. Not `.link`, so AppKit doesn't style links or
    /// open them on a plain click: they look like text until ⌘ is held.
    static let chatLink = NSAttributedString.Key("net.chillincool.backroom.chatLink")
}

/// Selectable message text. Links look like the text around them; with ⌘
/// held, the one under the mouse is underlined in the accent colour, the
/// cursor becomes a pointing hand, and a click opens it. Resting the mouse
/// on a web link shows `preview`'s view in a popover.
struct ChatTextView: NSViewRepresentable {
    let text: NSAttributedString
    let accent: NSColor
    var preview: ((URL) -> NSViewController)?
    let open: (URL) -> Void

    func makeNSView(context: Context) -> LinkTextView {
        let view = LinkTextView(usingTextLayoutManager: false)
        view.isEditable = false
        view.isSelectable = true
        view.drawsBackground = false
        view.textContainerInset = .zero
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.isVerticallyResizable = false
        view.isHorizontallyResizable = false
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return view
    }

    func updateNSView(_ view: LinkTextView, context: Context) {
        view.accent = accent
        view.open = open
        view.preview = preview
        if view.textStorage?.isEqual(to: text) != true {
            view.textStorage?.setAttributedString(text)
            view.hovered = nil
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView view: LinkTextView, context: Context) -> CGSize? {
        // Measured off to the side: resizing the live view's text container
        // here made SwiftUI measure again, forever.
        let size = TextMeasurer.size(text, width: proposal.width)
        return CGSize(width: proposal.width ?? size.width, height: size.height)
    }
}

/// Sizes message text the way `LinkTextView` lays it out, with one shared
/// TextKit stack and a cache.
@MainActor
enum TextMeasurer {
    private static let storage = NSTextStorage()
    private static let layout = NSLayoutManager()
    private static let container: NSTextContainer = {
        let container = NSTextContainer(size: NSSize(width: 1, height: CGFloat.greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        layout.addTextContainer(container)
        storage.addLayoutManager(layout)
        return container
    }()
    private struct Key: Hashable {
        let text: NSAttributedString
        let width: CGFloat?
    }
    private static var cache: [Key: CGSize] = [:]

    static func size(_ text: NSAttributedString, width: CGFloat?) -> CGSize {
        let width = width.map { max(1, $0.rounded(.down)) }
        let key = Key(text: text, width: width)
        if let size = cache[key] { return size }
        container.size = NSSize(width: width ?? .greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        storage.setAttributedString(text)
        layout.ensureLayout(for: container)
        let used = layout.usedRect(for: container)
        let size = CGSize(width: ceil(used.width), height: ceil(used.height))
        if cache.count > 4000 { cache.removeAll(keepingCapacity: true) }
        cache[key] = size
        return size
    }
}

final class LinkTextView: NSTextView {
    var accent: NSColor = .controlAccentColor
    var open: (URL) -> Void = { _ in }
    var preview: ((URL) -> NSViewController)?
    /// The web link the mouse rests on, and the popover showing its preview.
    private var resting: NSRange?
    private var restTimer: Timer?
    private var popover: NSPopover?
    private static let restDelay: TimeInterval = 0.6

    /// SwiftUI sizes the view (`TextMeasurer`); AppKit mustn't ask for more.
    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: NSView.noIntrinsicMetric)
    }
    /// The link under the mouse while ⌘ is held.
    var hovered: NSRange? {
        didSet {
            guard hovered != oldValue, let layout = layoutManager else { return }
            if let old = oldValue, old.upperBound <= (textStorage?.length ?? 0) {
                layout.removeTemporaryAttribute(.underlineStyle, forCharacterRange: old)
                layout.removeTemporaryAttribute(.foregroundColor, forCharacterRange: old)
            }
            if let new = hovered {
                layout.addTemporaryAttributes(
                    [.underlineStyle: NSUnderlineStyle.single.rawValue, .foregroundColor: accent],
                    forCharacterRange: new)
            }
        }
    }
    private var mouseInside = false
    private var flagsMonitor: Any?

    /// For lining the first line up with the nick and time beside it.
    override var firstBaselineOffsetFromTop: CGFloat {
        guard let layout = layoutManager, layout.numberOfGlyphs > 0 else {
            return font.map { $0.ascender } ?? 0
        }
        let line = layout.lineFragmentRect(forGlyphAt: 0, effectiveRange: nil)
        return line.minY + layout.location(forGlyphAt: 0).y
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas where area.owner === self && area.userInfo?["links"] != nil {
            removeTrackingArea(area)
        }
        addTrackingArea(NSTrackingArea(
            rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeInActiveApp, .inVisibleRect],
            owner: self, userInfo: ["links": true]))
    }

    override func mouseEntered(with event: NSEvent) {
        super.mouseEntered(with: event)
        mouseInside = true
        // ⌘ can go down or up while the mouse rests here.
        flagsMonitor = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { [weak self] event in
            self?.updateHover(event)
            return event
        }
        updateHover(event)
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        mouseInside = false
        if let flagsMonitor { NSEvent.removeMonitor(flagsMonitor) }
        flagsMonitor = nil
        hovered = nil
        rest(on: nil)
    }

    override func scrollWheel(with event: NSEvent) {
        rest(on: nil)
        super.scrollWheel(with: event)
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        updateHover(event)
    }

    override func cursorUpdate(with event: NSEvent) {
        if hovered != nil { NSCursor.pointingHand.set() } else { super.cursorUpdate(with: event) }
    }

    override func mouseDown(with event: NSEvent) {
        rest(on: nil)
        if event.modifierFlags.contains(.command), let (url, _) = link(at: event) {
            open(url)
            return
        }
        super.mouseDown(with: event)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil {
            if let flagsMonitor { NSEvent.removeMonitor(flagsMonitor) }
            flagsMonitor = nil
            rest(on: nil)
        }
    }

    private func updateHover(_ event: NSEvent) {
        let link = mouseInside ? link(at: event) : nil
        if event.type != .flagsChanged {
            rest(on: link.flatMap { ["http", "https"].contains($0.0.scheme?.lowercased()) ? $0 : nil })
        }
        guard event.modifierFlags.contains(.command), let (_, range) = link else {
            if hovered != nil {
                hovered = nil
                NSCursor.iBeam.set()
            }
            return
        }
        hovered = range
        NSCursor.pointingHand.set()
    }

    /// After the mouse rests on a web link for a moment, its preview pops
    /// up under it; moving off the link closes it.
    private func rest(on link: (URL, NSRange)?) {
        guard link?.1 != resting else { return }
        resting = link?.1
        restTimer?.invalidate()
        restTimer = nil
        popover?.close()
        popover = nil
        guard let (url, range) = link, preview != nil else { return }
        restTimer = Timer.scheduledTimer(withTimeInterval: Self.restDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.showPreview(url, range: range) }
        }
    }

    private func showPreview(_ url: URL, range: NSRange) {
        guard resting == range, window != nil, let preview, let layout = layoutManager,
            let container = textContainer
        else { return }
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        // A link wrapped over lines: point at its first line.
        var rect = layout.boundingRect(forGlyphRange: glyphs, in: container)
        let firstLine = layout.lineFragmentRect(forGlyphAt: glyphs.location, effectiveRange: nil)
        rect = rect.intersection(firstLine).isEmpty ? rect : rect.intersection(firstLine)
        rect = rect.offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
        let popover = NSPopover()
        popover.behavior = .applicationDefined
        popover.animates = false
        popover.contentViewController = preview(url)
        popover.show(relativeTo: rect, of: self, preferredEdge: .maxY)
        self.popover = popover
    }

    /// The link under the mouse, if the mouse is on its text (not just on
    /// the same line).
    private func link(at event: NSEvent) -> (URL, NSRange)? {
        guard let layout = layoutManager, let container = textContainer, let storage = textStorage,
            storage.length > 0
        else { return nil }
        // Flags-changed events have no meaningful location in this view.
        let windowPoint = event.type == .flagsChanged
            ? (window?.mouseLocationOutsideOfEventStream ?? .zero) : event.locationInWindow
        let point = convert(windowPoint, from: nil)
        var fraction: CGFloat = 0
        let glyph = layout.glyphIndex(for: point, in: container, fractionOfDistanceThroughGlyph: &fraction)
        let rect = layout.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container)
        guard rect.contains(point) else { return nil }
        let index = layout.characterIndexForGlyph(at: glyph)
        guard index < storage.length else { return nil }
        var range = NSRange()
        guard let url = storage.attribute(.chatLink, at: index, effectiveRange: &range) as? URL else { return nil }
        return (url, range)
    }
}
