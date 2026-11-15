//! Link previews: a card for a web link posted in chat (the page's title,
//! description and image), or the image itself for links to images.
//!
//! A posted link makes the client fetch it, so the fetcher is careful:
//! only `http`/`https`, a short timeout, few redirects, size caps, and no
//! private, loopback or link-local addresses (a link to `http://192.168.1.1/`
//! must not make the client poke the owner's router).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use scraper::{Html, Selector};
use url::{Host, Url};

/// Whole request, including redirects and reading the body.
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REDIRECTS: usize = 5;
/// Pages are read up to the end of `<head>`, at most this much.
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_TITLE_CHARS: usize = 300;
const MAX_DESCRIPTION_CHARS: usize = 500;

/// What a link shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    /// Where the link ended up after redirects.
    pub url: Url,
    pub kind: PreviewKind,
    pub title: Option<String>,
    pub description: Option<String>,
    /// `og:site_name`; front ends fall back to the host.
    pub site_name: Option<String>,
    /// The page's image (`og:image`), or the image a link points at.
    pub image: Option<PreviewImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// A web page: title, description, maybe an image.
    Page,
    /// The link is an image.
    Image,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreviewImage {
    pub url: Url,
    /// `Content-Type`, e.g. `image/png`.
    pub mime: String,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for PreviewImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewImage")
            .field("url", &self.url.as_str())
            .field("mime", &self.mime)
            .field("bytes", &self.data.len())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("not an http(s) link")]
    Scheme,
    #[error("refusing to fetch a private or local address")]
    Blocked,
    #[error("{0}")]
    Http(String),
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("too large")]
    TooLarge,
    #[error("unsupported content type {0:?}")]
    Unsupported(String),
    /// A page without a title or image: nothing worth showing.
    #[error("nothing to preview")]
    Empty,
}

/// Fetches previews. Cheap to clone; share one.
#[derive(Clone)]
pub struct Previewer {
    http: reqwest::Client,
    allow_private: bool,
}

impl Previewer {
    pub fn new() -> Result<Self, PreviewError> {
        Self::build(false)
    }

    fn build(allow_private: bool) -> Result<Self, PreviewError> {
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error(PreviewError::Http("too many redirects".into()))
            } else if let Err(e) = check_url(attempt.url(), allow_private) {
                attempt.error(e)
            } else {
                attempt.follow()
            }
        });
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(policy)
            // A proxy would resolve names itself, around the address check.
            .no_proxy()
            .dns_resolver(Arc::new(PublicResolver { allow_private }))
            // Some sites only give link-preview bots their OpenGraph tags.
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; Backroom/",
                env!("CARGO_PKG_VERSION"),
                "; link preview)"
            ))
            .build()
            .map_err(|e| PreviewError::Http(e.to_string()))?;
        Ok(Self {
            http,
            allow_private,
        })
    }

    /// Fetches `url` and, for a page with an image, that image.
    pub async fn preview(&self, url: &str) -> Result<LinkPreview, PreviewError> {
        let url = Url::parse(url).map_err(|_| PreviewError::Scheme)?;
        check_url(&url, self.allow_private)?;
        let response = self
            .http
            .get(url)
            // Not "text/html first": i.imgur.com then redirects image links
            // to the image's page.
            .header(ACCEPT, HeaderValue::from_static("*/*"))
            .send()
            .await
            .map_err(http_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(PreviewError::Status(status.as_u16()));
        }
        let final_url = response.url().clone();
        let mime = mime_of(&response);
        if mime.starts_with("image/") {
            let data = read_capped(response, MAX_IMAGE_BYTES, false).await?;
            return Ok(LinkPreview {
                url: final_url.clone(),
                kind: PreviewKind::Image,
                title: None,
                description: None,
                site_name: None,
                image: Some(PreviewImage {
                    url: final_url,
                    mime,
                    data,
                }),
            });
        }
        if mime != "text/html" && mime != "application/xhtml+xml" {
            return Err(PreviewError::Unsupported(mime));
        }
        let head = read_capped(response, MAX_PAGE_BYTES, true).await?;
        let meta = parse_page(&String::from_utf8_lossy(&head), &final_url);
        if meta.title.is_none() && meta.image.is_none() {
            return Err(PreviewError::Empty);
        }
        // A broken image still leaves a useful card.
        let image = match &meta.image {
            Some(image_url) => self.image(image_url).await.ok(),
            None => None,
        };
        if meta.title.is_none() && image.is_none() {
            return Err(PreviewError::Empty);
        }
        Ok(LinkPreview {
            url: final_url,
            kind: PreviewKind::Page,
            title: meta.title,
            description: meta.description,
            site_name: meta.site_name,
            image,
        })
    }

    async fn image(&self, url: &Url) -> Result<PreviewImage, PreviewError> {
        check_url(url, self.allow_private)?;
        let response = self
            .http
            .get(url.clone())
            .header(ACCEPT, HeaderValue::from_static("image/*"))
            .send()
            .await
            .map_err(http_error)?;
        if !response.status().is_success() {
            return Err(PreviewError::Status(response.status().as_u16()));
        }
        let mime = mime_of(&response);
        if !mime.starts_with("image/") {
            return Err(PreviewError::Unsupported(mime));
        }
        let url = response.url().clone();
        let data = read_capped(response, MAX_IMAGE_BYTES, false).await?;
        Ok(PreviewImage { url, mime, data })
    }

    #[cfg(test)]
    fn allowing_private() -> Self {
        Self::build(true).unwrap()
    }
}

fn http_error(e: reqwest::Error) -> PreviewError {
    // Errors from the redirect policy and the resolver come back wrapped.
    let mut source: Option<&dyn std::error::Error> = Some(&e);
    while let Some(err) = source {
        if let Some(PreviewError::Blocked) = err.downcast_ref::<PreviewError>() {
            return PreviewError::Blocked;
        }
        source = err.source();
    }
    PreviewError::Http(e.to_string())
}

/// The media type without parameters, lowercased.
fn mime_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Reads the body up to `max` bytes. `head_only`: stop after `</head>` (and
/// keep what was read when the cap is hit); otherwise exceeding the cap fails.
async fn read_capped(
    mut response: reqwest::Response,
    max: usize,
    head_only: bool,
) -> Result<Vec<u8>, PreviewError> {
    if !head_only && response.content_length().is_some_and(|n| n > max as u64) {
        return Err(PreviewError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
        let searched_from = body.len().saturating_sub(7);
        body.extend_from_slice(&chunk);
        if head_only && contains_head_end(&body[searched_from..]) {
            break;
        }
        if body.len() > max {
            if head_only {
                body.truncate(max);
                break;
            }
            return Err(PreviewError::TooLarge);
        }
    }
    Ok(body)
}

fn contains_head_end(bytes: &[u8]) -> bool {
    bytes.windows(7).any(|w| w.eq_ignore_ascii_case(b"</head>"))
}

/// Only `http`/`https`, and no literal private address.
fn check_url(url: &Url, allow_private: bool) -> Result<(), PreviewError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(PreviewError::Scheme);
    }
    let ip = match url.host() {
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if !allow_private && (d == "localhost" || d.ends_with(".localhost")) {
                return Err(PreviewError::Blocked);
            }
            return Ok(());
        }
        None => return Err(PreviewError::Scheme),
    };
    if allow_private || is_public(ip) {
        Ok(())
    } else {
        Err(PreviewError::Blocked)
    }
}

/// Resolves names and drops addresses that aren't public. Literal IPs in
/// URLs skip the resolver; `check_url` covers them.
struct PublicResolver {
    allow_private: bool,
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
                .await?
                .filter(|a| allow_private || is_public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(PreviewError::Blocked) as Box<_>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Whether an address is on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 (64:ff9b::/96) embeds an IPv4 address.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return is_public_v4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                ));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 240) // reserved
}

/// What a page's `<head>` says about it.
#[derive(Debug, Default, PartialEq, Eq)]
struct PageMeta {
    title: Option<String>,
    description: Option<String>,
    site_name: Option<String>,
    image: Option<Url>,
}

fn parse_page(html: &str, base: &Url) -> PageMeta {
    let doc = Html::parse_document(html);
    let meta = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|key| {
            let selector =
                Selector::parse(&format!("meta[property=\"{key}\"], meta[name=\"{key}\"]")).ok()?;
            doc.select(&selector)
                .filter_map(|e| e.value().attr("content"))
                .map(clean)
                .find(|s| !s.is_empty())
        })
    };
    let title = meta(&["og:title", "twitter:title"]).or_else(|| {
        let selector = Selector::parse("title").ok()?;
        doc.select(&selector)
            .map(|e| clean(&e.text().collect::<String>()))
            .find(|s| !s.is_empty())
    });
    let image = meta(&[
        "og:image:secure_url",
        "og:image",
        "og:image:url",
        "twitter:image",
        "twitter:image:src",
    ])
    .and_then(|s| base.join(&s).ok())
    .filter(|u| matches!(u.scheme(), "http" | "https"));
    PageMeta {
        title: title.map(|t| truncate(t, MAX_TITLE_CHARS)),
        description: meta(&["og:description", "twitter:description", "description"])
            .map(|d| truncate(d, MAX_DESCRIPTION_CHARS)),
        site_name: meta(&["og:site_name"]).map(|s| truncate(s, MAX_TITLE_CHARS)),
        image,
    }
}

/// Collapses whitespace.
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn reads_opengraph_tags() {
        let html = r#"<html><head>
            <title>Fallback</title>
            <meta property="og:title" content="  The   Title &amp; more ">
            <meta property="og:description" content="What it's about.">
            <meta property="og:site_name" content="Example">
            <meta property="og:image" content="/img/card.png">
            </head><body>ignored</body></html>"#;
        let meta = parse_page(html, &url("https://example.com/a/b"));
        assert_eq!(
            meta,
            PageMeta {
                title: Some("The Title & more".into()),
                description: Some("What it's about.".into()),
                site_name: Some("Example".into()),
                image: Some(url("https://example.com/img/card.png")),
            }
        );
    }

    #[test]
    fn falls_back_to_twitter_tags_and_title() {
        let html = r#"<head><title>
              Plain title </title>
            <meta name="twitter:image" content="https://cdn.example.com/x.jpg">
            <meta name="description" content="Desc"></head>"#;
        let meta = parse_page(html, &url("https://example.com/"));
        assert_eq!(meta.title.as_deref(), Some("Plain title"));
        assert_eq!(meta.description.as_deref(), Some("Desc"));
        assert_eq!(meta.image, Some(url("https://cdn.example.com/x.jpg")));
        assert_eq!(meta.site_name, None);
    }

    #[test]
    fn ignores_non_http_images_and_truncates() {
        let long = "x".repeat(MAX_TITLE_CHARS + 50);
        let html = format!(
            r#"<head><meta property="og:title" content="{long}">
            <meta property="og:image" content="javascript:alert(1)"></head>"#
        );
        let meta = parse_page(&html, &url("https://example.com/"));
        assert_eq!(meta.image, None);
        let title = meta.title.unwrap();
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn private_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.1.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700::1111",
            "64:ff9b::101:101",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn checks_urls() {
        let check = |s: &str| check_url(&url(s), false);
        assert!(check("https://example.com/").is_ok());
        assert!(matches!(
            check("ftp://example.com/"),
            Err(PreviewError::Scheme)
        ));
        assert!(matches!(
            check("http://127.0.0.1:8080/"),
            Err(PreviewError::Blocked)
        ));
        assert!(matches!(check("http://[::1]/"), Err(PreviewError::Blocked)));
        assert!(matches!(
            check("http://localhost/"),
            Err(PreviewError::Blocked)
        ));
        assert!(matches!(
            check("http://a.localhost./"),
            Err(PreviewError::Blocked)
        ));
    }

    #[tokio::test]
    async fn blocks_names_resolving_to_private_addresses() {
        let previewer = Previewer::new().unwrap();
        // Literal addresses are checked up front, resolved names by the resolver.
        let err = previewer.preview("http://127.0.0.1:9/").await.unwrap_err();
        assert!(matches!(err, PreviewError::Blocked), "{err:?}");
        let resolved = PublicResolver {
            allow_private: false,
        }
        .resolve("localhost".parse().unwrap())
        .await;
        assert!(resolved.is_err());
    }

    /// Serves canned responses: path → (status line, content type, body).
    async fn serve(routes: Vec<(&'static str, &'static str, &'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (status, extra, body) = match routes.iter().find(|r| r.0 == path) {
                        Some((_, status, extra, body)) => (*status, *extra, body.clone()),
                        None => ("404 Not Found", "Content-Type: text/plain", b"no".to_vec()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\n{extra}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn previews_a_page_with_its_image() {
        let page = br#"<html><head><meta property="og:title" content="Hello">
            <meta property="og:image" content="/card.png"></head><body></body></html>"#;
        let base = serve(vec![
            ("/start", "302 Found", "Location: /page", vec![]),
            (
                "/page",
                "200 OK",
                "Content-Type: text/html; charset=utf-8",
                page.to_vec(),
            ),
            (
                "/card.png",
                "200 OK",
                "Content-Type: image/png",
                vec![1, 2, 3],
            ),
        ])
        .await;
        let preview = Previewer::allowing_private()
            .preview(&format!("{base}/start"))
            .await
            .unwrap();
        assert_eq!(preview.url.path(), "/page");
        assert_eq!(preview.kind, PreviewKind::Page);
        assert_eq!(preview.title.as_deref(), Some("Hello"));
        let image = preview.image.unwrap();
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.data, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn previews_an_image_link() {
        let base = serve(vec![(
            "/pic.jpg",
            "200 OK",
            "Content-Type: image/jpeg",
            vec![9; 10],
        )])
        .await;
        let preview = Previewer::allowing_private()
            .preview(&format!("{base}/pic.jpg"))
            .await
            .unwrap();
        assert_eq!(preview.kind, PreviewKind::Image);
        assert_eq!(preview.image.unwrap().data.len(), 10);
    }

    #[tokio::test]
    async fn rejects_large_images_errors_and_other_types() {
        let base = serve(vec![
            (
                "/big.png",
                "200 OK",
                "Content-Type: image/png",
                vec![0; MAX_IMAGE_BYTES + 1],
            ),
            (
                "/file.zip",
                "200 OK",
                "Content-Type: application/zip",
                vec![0; 4],
            ),
            (
                "/empty",
                "200 OK",
                "Content-Type: text/html",
                b"<p>hi</p>".to_vec(),
            ),
        ])
        .await;
        let previewer = Previewer::allowing_private();
        let err = |path: &str| {
            let previewer = previewer.clone();
            let url = format!("{base}{path}");
            async move { previewer.preview(&url).await.unwrap_err() }
        };
        assert!(matches!(err("/big.png").await, PreviewError::TooLarge));
        assert!(matches!(
            err("/file.zip").await,
            PreviewError::Unsupported(_)
        ));
        assert!(matches!(err("/empty").await, PreviewError::Empty));
        assert!(matches!(err("/missing").await, PreviewError::Status(404)));
    }

    #[tokio::test]
    async fn stops_reading_after_head() {
        let mut page = br#"<html><head><title>Big page</title></head><body>"#.to_vec();
        page.extend(std::iter::repeat_n(b'x', 2 * MAX_PAGE_BYTES));
        let base = serve(vec![("/", "200 OK", "Content-Type: text/html", page)]).await;
        let preview = Previewer::allowing_private()
            .preview(&format!("{base}/"))
            .await
            .unwrap();
        assert_eq!(preview.title.as_deref(), Some("Big page"));
    }
}
