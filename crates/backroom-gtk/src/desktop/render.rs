//! Message text as Pango markup: mIRC formatting, web links and channel
//! names (both look like the text around them; Ctrl-click opens them).

use backroom_app::links::{self, LinkTarget};
use backroom_ffi::FormattedSpan;
use gtk::glib;

use crate::desktop::style::Palette;

/// The `href` scheme of channel links.
pub const JOIN_SCHEME: &str = "backroom-join:";

/// Markup for message text in the default colour `color`.
pub fn message_markup(text: &str, formatting: bool, palette: &Palette, italic: bool) -> String {
    let spans: Vec<FormattedSpan> = if formatting {
        backroom_ffi::format_spans(text.to_owned())
    } else {
        vec![plain_span(backroom_ffi::strip_formatting(text.to_owned()))]
    };
    let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
    let found = links::find(&plain);

    let mut out = String::new();
    // Walk the text once, cutting at span and link boundaries.
    let mut span_starts = Vec::with_capacity(spans.len());
    let mut offset = 0;
    for s in &spans {
        span_starts.push(offset);
        offset += s.text.len();
    }
    let mut cuts: Vec<usize> = span_starts.clone();
    for l in &found {
        cuts.push(l.start);
        cuts.push(l.end);
    }
    cuts.push(plain.len());
    cuts.sort_unstable();
    cuts.dedup();

    let mut link_open: Option<usize> = None;
    for window in cuts.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start >= end {
            continue;
        }
        // Close or open links at this cut.
        if let Some(i) = link_open
            && found[i].end <= start
        {
            out.push_str("</a>");
            link_open = None;
        }
        if link_open.is_none()
            && let Some(i) = found.iter().position(|l| l.start == start)
        {
            let href = match &found[i].target {
                LinkTarget::Web(url) => url.clone(),
                LinkTarget::Channel(name) => format!("{JOIN_SCHEME}{name}"),
            };
            out.push_str(&format!("<a href=\"{}\">", glib::markup_escape_text(&href)));
            link_open = Some(i);
        }
        let si = span_starts
            .iter()
            .rposition(|&s| s <= start)
            .unwrap_or_default();
        let span = &spans[si];
        let piece = &plain[start..end];
        out.push_str(&span_markup(span, piece, palette, italic));
    }
    if link_open.is_some() {
        out.push_str("</a>");
    }
    out
}

fn plain_span(text: String) -> FormattedSpan {
    FormattedSpan {
        text,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        monospace: false,
        reverse: false,
        fg: None,
        bg: None,
    }
}

fn rgb(c: u32) -> String {
    format!("#{c:06x}")
}

fn span_markup(span: &FormattedSpan, text: &str, palette: &Palette, italic: bool) -> String {
    let mut attrs = Vec::new();
    if span.bold {
        attrs.push("weight=\"bold\"".to_owned());
    }
    if italic || span.italic {
        attrs.push("style=\"italic\"".to_owned());
    }
    if span.underline {
        attrs.push("underline=\"single\"".to_owned());
    }
    if span.strikethrough {
        attrs.push("strikethrough=\"true\"".to_owned());
    }
    if span.monospace {
        attrs.push("font_family=\"monospace\"".to_owned());
    }
    let (mut fg, mut bg) = (span.fg.map(rgb), span.bg.map(rgb));
    if span.reverse {
        (fg, bg) = (
            Some(bg.unwrap_or_else(|| palette.background.clone())),
            Some(fg.unwrap_or_else(|| palette.text_or_default())),
        );
    }
    if let Some(fg) = fg {
        attrs.push(format!("foreground=\"{fg}\""));
    }
    if let Some(bg) = bg {
        attrs.push(format!("background=\"{bg}\""));
    }
    let escaped = glib::markup_escape_text(text);
    if attrs.is_empty() {
        escaped.to_string()
    } else {
        format!("<span {}>{escaped}</span>", attrs.join(" "))
    }
}

/// A plain run of text in a colour, optionally bold or italic.
pub fn run(text: &str, color: Option<&str>, bold: bool, italic: bool) -> String {
    let mut attrs = Vec::new();
    if let Some(c) = color {
        attrs.push(format!("foreground=\"{c}\""));
    }
    if bold {
        attrs.push("weight=\"bold\"".into());
    }
    if italic {
        attrs.push("style=\"italic\"".into());
    }
    let escaped = glib::markup_escape_text(text);
    if attrs.is_empty() {
        escaped.to_string()
    } else {
        format!("<span {}>{escaped}</span>", attrs.join(" "))
    }
}

/// The channel a join link points at.
pub fn channel_of(uri: &str) -> Option<&str> {
    uri.strip_prefix(JOIN_SCHEME).filter(|c| !c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use backroom_app::theme::Theme;

    fn palette() -> Palette {
        let probe = crate::desktop::style::Probe {
            background: "#ffffff".into(),
            muted: "#777777".into(),
            accent: "#3584e4".into(),
        };
        Palette::new(&Theme::default_light(), false, &probe)
    }

    #[test]
    fn links_wrap_formatted_spans() {
        let markup = message_markup(
            "see \u{2}https://a.example/x?a=1&b=2\u{2} & #chan",
            true,
            &palette(),
            false,
        );
        assert_eq!(
            markup,
            "see <a href=\"https://a.example/x?a=1&amp;b=2\"><span weight=\"bold\">https://a.example/x?a=1&amp;b=2</span></a> &amp; <a href=\"backroom-join:#chan\">#chan</a>"
        );
    }

    #[test]
    fn colours_and_reverse() {
        let markup = message_markup("\u{3}4red\u{f} \u{16}rev", true, &palette(), false);
        assert!(markup.contains("foreground=\"#ff0000\""), "{markup}");
        assert!(
            markup.contains("foreground=\"#ffffff\" background=\"#1e1e1e\""),
            "{markup}"
        );
        let stripped = message_markup("\u{3}4red", false, &palette(), true);
        assert_eq!(stripped, "<span style=\"italic\">red</span>");
    }

    #[test]
    fn join_links() {
        assert_eq!(channel_of("backroom-join:#rust"), Some("#rust"));
        assert_eq!(channel_of("https://x"), None);
    }
}
