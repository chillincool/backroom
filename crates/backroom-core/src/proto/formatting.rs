//! mIRC-style formatting codes (<https://modern.ircdocs.horse/formatting>).
//!
//! For now this only strips them; a styled-span parser for the UI comes later.

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

#[cfg(test)]
mod tests {
    use super::*;

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
