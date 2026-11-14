//! mIRC-style formatting codes (<https://modern.ircdocs.horse/formatting>):
//! stripped, or parsed into styled spans for display.

const BOLD: char = '\x02';
const COLOR: char = '\x03';
const HEX_COLOR: char = '\x04';
const RESET: char = '\x0f';
const MONOSPACE: char = '\x11';
const REVERSE: char = '\x16';
const ITALIC: char = '\x1d';
const STRIKETHROUGH: char = '\x1e';
const UNDERLINE: char = '\x1f';

/// Removes all formatting codes, including color arguments.
pub fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            BOLD | RESET | MONOSPACE | REVERSE | ITALIC | STRIKETHROUGH | UNDERLINE => {}
            COLOR => skip_color_args(&mut chars, 2, |c| c.is_ascii_digit()),
            HEX_COLOR => skip_color_args(&mut chars, 6, |c| c.is_ascii_hexdigit()),
            c => out.push(c),
        }
    }
    out
}

/// Skips `<fg>[,<bg>]` where each part is up to `max` chars matching `valid`.
/// A comma is only consumed when a background color follows it.
fn skip_color_args(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    max: usize,
    valid: impl Fn(char) -> bool,
) {
    let take = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        let mut n = 0;
        while n < max && chars.peek().is_some_and(|&c| valid(c)) {
            chars.next();
            n += 1;
        }
        n
    };
    if take(chars) == 0 {
        return;
    }
    if chars.peek() == Some(&',') {
        let mut lookahead = chars.clone();
        lookahead.next();
        if lookahead.peek().is_some_and(|&c| valid(c)) {
            chars.next();
            take(chars);
        }
    }
}

/// Text style at some point in a message. Colours are `0xRRGGBB`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub monospace: bool,
    /// Swap foreground and background.
    pub reverse: bool,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
}

impl Style {
    pub fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// Splits text into styled spans, dropping the codes. Adjacent spans in
/// the same style are merged; plain text is one plain span.
pub fn parse_formatting(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut chars = text.chars().peekable();
    let push = |c: char, style: Style, spans: &mut Vec<Span>| match spans.last_mut() {
        Some(last) if last.style == style => last.text.push(c),
        _ => spans.push(Span {
            text: c.to_string(),
            style,
        }),
    };
    while let Some(c) = chars.next() {
        match c {
            BOLD => style.bold = !style.bold,
            ITALIC => style.italic = !style.italic,
            UNDERLINE => style.underline = !style.underline,
            STRIKETHROUGH => style.strikethrough = !style.strikethrough,
            MONOSPACE => style.monospace = !style.monospace,
            REVERSE => style.reverse = !style.reverse,
            RESET => style = Style::default(),
            COLOR => match color_args(&mut chars, 2, |c| c.is_ascii_digit()) {
                None => (style.fg, style.bg) = (None, None),
                Some((fg, bg)) => {
                    style.fg = palette(&fg);
                    if let Some(bg) = bg {
                        style.bg = palette(&bg);
                    }
                }
            },
            HEX_COLOR => match color_args(&mut chars, 6, |c| c.is_ascii_hexdigit()) {
                None => (style.fg, style.bg) = (None, None),
                Some((fg, bg)) => {
                    style.fg = hex(&fg);
                    if let Some(bg) = bg {
                        style.bg = hex(&bg);
                    }
                }
            },
            c => push(c, style, &mut spans),
        }
    }
    spans
}

/// Reads `<fg>[,<bg>]` like [`skip_color_args`]; `None` when no colour
/// follows (which resets the colours).
fn color_args(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    max: usize,
    valid: impl Fn(char) -> bool,
) -> Option<(String, Option<String>)> {
    let take = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        let mut out = String::new();
        while out.len() < max && chars.peek().is_some_and(|&c| valid(c)) {
            out.push(chars.next().unwrap());
        }
        out
    };
    let fg = take(chars);
    if fg.is_empty() {
        return None;
    }
    let mut bg = None;
    if chars.peek() == Some(&',') {
        let mut lookahead = chars.clone();
        lookahead.next();
        if lookahead.peek().is_some_and(|&c| valid(c)) {
            chars.next();
            bg = Some(take(chars));
        }
    }
    Some((fg, bg))
}

fn hex(code: &str) -> Option<u32> {
    (code.len() == 6)
        .then(|| u32::from_str_radix(code, 16).ok())
        .flatten()
}

/// mIRC colours 0–98 (99 means the default colour).
fn palette(code: &str) -> Option<u32> {
    PALETTE.get(code.parse::<usize>().ok()?).copied()
}

/// <https://modern.ircdocs.horse/formatting#colors-16-98>
const PALETTE: [u32; 99] = [
    0xffffff, 0x000000, 0x00007f, 0x009300, 0xff0000, 0x7f0000, 0x9c009c, 0xfc7f00, 0xffff00,
    0x00fc00, 0x009393, 0x00ffff, 0x0000fc, 0xff00ff, 0x7f7f7f, 0xd2d2d2, //
    0x470000, 0x472100, 0x474700, 0x324700, 0x004700, 0x00472c, 0x004747, 0x002747, 0x000047,
    0x2e0047, 0x470047, 0x47002a, //
    0x740000, 0x743a00, 0x747400, 0x517400, 0x007400, 0x007449, 0x007474, 0x004074, 0x000074,
    0x4b0074, 0x740074, 0x740045, //
    0xb50000, 0xb56300, 0xb5b500, 0x7db500, 0x00b500, 0x00b571, 0x00b5b5, 0x0063b5, 0x0000b5,
    0x7500b5, 0xb500b5, 0xb5006b, //
    0xff0000, 0xff8c00, 0xffff00, 0xb2ff00, 0x00ff00, 0x00ffa0, 0x00ffff, 0x008cff, 0x0000ff,
    0xa500ff, 0xff00ff, 0xff0098, //
    0xff5959, 0xffb459, 0xffff71, 0xcfff60, 0x6fff6f, 0x65ffc9, 0x6dffff, 0x59b4ff, 0x5959ff,
    0xc459ff, 0xff66ff, 0xff59bc, //
    0xff9c9c, 0xffd39c, 0xffff9c, 0xe2ff9c, 0x9cff9c, 0x9cffdb, 0x9cffff, 0x9cd3ff, 0x9c9cff,
    0xdc9cff, 0xff9cff, 0xff94d3, //
    0x000000, 0x131313, 0x282828, 0x363636, 0x4d4d4d, 0x656565, 0x818181, 0x9f9f9f, 0xbcbcbc,
    0xe2e2e2, 0xffffff,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(text: &str) -> Vec<(String, Style)> {
        parse_formatting(text)
            .into_iter()
            .map(|s| (s.text, s.style))
            .collect()
    }

    #[test]
    fn plain_text_is_one_plain_span() {
        assert_eq!(
            styles("hello there"),
            [("hello there".into(), Style::default())]
        );
        assert!(parse_formatting("").is_empty());
    }

    #[test]
    fn toggles_colours_and_reset() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let red_on_blue = Style {
            bold: true,
            fg: Some(0xff0000),
            bg: Some(0x0000fc),
            ..Style::default()
        };
        assert_eq!(
            styles("a\x02b\x0304,12c\x03d\x0fe"),
            [
                ("a".into(), Style::default()),
                ("b".into(), bold),
                ("c".into(), red_on_blue),
                ("d".into(), bold),
                ("e".into(), Style::default()),
            ]
        );
        // 99 is the default colour; a comma without a colour is text.
        assert_eq!(styles("\x0399x")[0].1.fg, None);
        assert_eq!(
            styles("\x034,x"),
            [(
                ",x".into(),
                Style {
                    fg: Some(0xff0000),
                    ..Style::default()
                }
            )]
        );
        assert_eq!(styles("\x04FF00AAhex")[0].1.fg, Some(0xff00aa));
        assert_eq!(styles("\x0388x")[0].1.fg, Some(0x000000));
    }

    #[test]
    fn strips_toggles() {
        assert_eq!(
            strip_formatting("\x02bold\x02 \x1ditalic\x0f"),
            "bold italic"
        );
    }

    #[test]
    fn strips_colors_with_arguments() {
        assert_eq!(strip_formatting("\x0304red\x03 plain"), "red plain");
        assert_eq!(strip_formatting("\x0304,12on blue"), "on blue");
        assert_eq!(strip_formatting("\x034,5x"), "x");
        assert_eq!(strip_formatting("\x0312345"), "345");
        assert_eq!(strip_formatting("\x0304,text"), ",text");
        assert_eq!(strip_formatting("\x04FF00AA,000000hex"), "hex");
    }
}
