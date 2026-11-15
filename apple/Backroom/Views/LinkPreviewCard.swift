import AppKit
import SwiftUI

extension EnvironmentValues {
    /// Nil under tests and when the previewer couldn't start.
    @Entry var linkPreviews: LinkPreviews?
}

/// What pops up when the mouse rests on a link: the linked image, or the
/// page's site, title, description and image. The link is fetched only
/// then (fetching shows the site our IP address).
struct LinkPreviewCard: View {
    let url: URL
    let previews: LinkPreviews
    let style: ChatStyle
    @State private var entry: LinkPreviews.Entry?

    private var theme: Theme { style.theme }
    private static let maxImage = CGSize(width: 360, height: 270)

    /// For `ChatTextView`'s popover.
    @MainActor static func controller(url: URL, previews: LinkPreviews, style: ChatStyle) -> NSViewController {
        let controller = NSHostingController(rootView: LinkPreviewCard(url: url, previews: previews, style: style))
        controller.sizingOptions = .preferredContentSize
        controller.view.appearance = NSAppearance(named: style.theme.isDark ? .darkAqua : .aqua)
        return controller
    }

    var body: some View {
        Group {
            switch entry?.state {
            case .loaded(let preview): card(preview)
            case .failed: status("No preview")
            case .loading, nil: status(nil)
            }
        }
        .padding(10)
        .task(id: url) { entry = previews.entry(for: url) }
    }

    /// Loading (a spinner) or nothing to show, with the host.
    private func status(_ text: String?) -> some View {
        HStack(spacing: 6) {
            if let text {
                Text(text).foregroundStyle(theme.mutedColor)
            } else {
                ProgressView().controlSize(.small)
            }
            Text(url.host(percentEncoded: false) ?? url.absoluteString)
                .foregroundStyle(theme.mutedColor)
                .lineLimit(1)
        }
        .font(style.smallFont)
    }

    @ViewBuilder private func card(_ preview: LinkPreviews.Preview) -> some View {
        switch preview.kind {
        case .image:
            if let image = preview.image { picture(image, max: Self.maxImage) }
        case .page:
            page(preview)
        }
    }

    private func page(_ preview: LinkPreviews.Preview) -> some View {
        // Wide images (video thumbnails, article headers) go under the
        // text; square-ish ones (logos, avatars) beside it.
        let wide = preview.image.map { $0.size.width >= $0.size.height * 1.3 && $0.size.width >= 200 } ?? false
        return HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(preview.site)
                    .font(style.smallFont)
                    .foregroundStyle(theme.mutedColor)
                    .lineLimit(1)
                if let title = preview.title {
                    Text(title)
                        .font(style.font.weight(.semibold))
                        .foregroundStyle(theme.textColor)
                        .lineLimit(3)
                }
                if let description = preview.description {
                    Text(description)
                        .font(style.smallFont)
                        .foregroundStyle(theme.mutedColor)
                        .lineLimit(4)
                }
                if wide, let image = preview.image {
                    picture(image, max: CGSize(width: 360, height: 200)).padding(.top, 4)
                }
            }
            .frame(maxWidth: 360, alignment: .leading)
            if !wide, let image = preview.image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 64, height: 64)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }
        }
        .multilineTextAlignment(.leading)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// The image at most `max`, never enlarged.
    private func picture(_ image: NSImage, max: CGSize) -> some View {
        let scale = min(1, max.width / Swift.max(image.size.width, 1), max.height / Swift.max(image.size.height, 1))
        return Image(nsImage: image)
            .resizable()
            .interpolation(.high)
            .frame(width: image.size.width * scale, height: image.size.height * scale)
            .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}
