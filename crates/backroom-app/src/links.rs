//! Web links and channel names in message text.

use linkify::{LinkFinder, LinkKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A URL to open in the browser.
    Web(String),
    /// A channel to join.
    Channel(String),
}

/// A link in plain text: byte offsets into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

const SCHEMES: &[&str] = &["http", "https", "mailto", "ftp", "irc", "ircs"];

/// Web links and `#channel` names in plain (unformatted) text, in order.
pub fn find(plain: &str) -> Vec<Link> {
    let mut links: Vec<Link> = LinkFinder::new()
        .kinds(&[LinkKind::Url])
        .links(plain)
        .filter_map(|l| {
            let scheme = l.as_str().split(':').next()?.to_lowercase();
            SCHEMES.contains(&scheme.as_str()).then(|| Link {
                start: l.start(),
                end: l.end(),
                target: LinkTarget::Web(l.as_str().to_owned()),
            })
        })
        .collect();
    // `#channel` at the start or after a space or bracket.
    let mut prev: Option<char> = None;
    for (i, c) in plain.char_indices() {
        let starts = c == '#' && prev.is_none_or(|p| p.is_whitespace() || p == '(' || p == '[');
        prev = Some(c);
        if !starts {
            continue;
        }
        let rest = &plain[i..];
        let len = rest
            .char_indices()
            .skip(1)
            .find(|&(_, c)| c.is_whitespace() || ",()[]\u{7}".contains(c))
            .map_or(rest.len(), |(j, _)| j);
        let name = rest[..len].trim_end_matches(['.', '!', '?', ':', ';', '\'', '"']);
        if name.len() < 2 {
            continue;
        }
        let (start, end) = (i, i + name.len());
        if links.iter().any(|l| start < l.end && l.start < end) {
            continue;
        }
        links.push(Link {
            start,
            end,
            target: LinkTarget::Channel(name.to_owned()),
        });
    }
    links.sort_by_key(|l| l.start);
    links
}

/// The first web link (http or https) in message text, mIRC formatting
/// included.
pub fn first_web_link(text: &str) -> Option<String> {
    let plain = backroom_ffi::strip_formatting(text.to_owned());
    find(&plain).into_iter().find_map(|l| match l.target {
        LinkTarget::Web(url) if url.starts_with("http") => Some(url),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_urls_and_channels() {
        let text = "see https://example.net/a#frag and #rust, (#go) or x#no #.";
        let links = find(text);
        let targets: Vec<_> = links.iter().map(|l| l.target.clone()).collect();
        assert_eq!(
            targets,
            [
                LinkTarget::Web("https://example.net/a#frag".into()),
                LinkTarget::Channel("#rust".into()),
                LinkTarget::Channel("#go".into()),
            ]
        );
        assert_eq!(&text[links[1].start..links[1].end], "#rust");
    }

    #[test]
    fn trailing_punctuation_isnt_part_of_a_channel() {
        let links = find("join #backroom!");
        assert_eq!(links[0].target, LinkTarget::Channel("#backroom".into()));
    }

    #[test]
    fn ignores_other_schemes() {
        assert!(find("javascript:alert(1) file:///etc/passwd").is_empty());
        assert_eq!(
            first_web_link("\u{3}4https://a.example/\u{3}"),
            Some("https://a.example/".into())
        );
    }
}
