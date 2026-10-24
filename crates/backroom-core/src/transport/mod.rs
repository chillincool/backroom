//! Line-oriented transports. Protocol code only ever sees whole IRC lines
//! without CRLF; framing (WebSocket messages, TCP `\r\n`) lives here.

pub mod tcp;
mod tls;
pub mod websocket;

#[cfg(test)]
pub(crate) mod mock;

use async_trait::async_trait;

pub use tcp::TcpConnector;
pub use tls::TlsOptions;
pub use websocket::WebSocketConnector;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("invalid server URL: {0}")]
    InvalidUrl(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("server sent a line longer than the limit")]
    LineTooLong,
    #[error("connection closed")]
    Closed,
}

impl From<tokio_tungstenite::tungstenite::Error> for TransportError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        TransportError::WebSocket(Box::new(e))
    }
}

/// An open connection carrying one IRC line per `send`/`recv`.
#[async_trait]
pub trait Transport: Send {
    /// Sends one line. `line` must not contain CR or LF.
    async fn send(&mut self, line: &str) -> Result<(), TransportError>;
    /// Receives one line, or `None` once the peer has closed the connection.
    async fn recv(&mut self) -> Result<Option<String>, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

/// Opens new transports; called once per (re)connect.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self) -> Result<Box<dyn Transport>, TransportError>;
}

/// Picks a transport from the URL scheme:
/// `wss://` / `ws://` (IRC over WebSocket), `ircs://` (TLS) or
/// `irc+insecure://` (plain text), matching soju's own `listen` syntax.
pub fn connector_for_url(url: &str, tls: TlsOptions) -> Result<Box<dyn Connector>, TransportError> {
    let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or_default();
    match scheme {
        "wss" | "ws" => Ok(Box::new(WebSocketConnector::with_tls(url, tls)?)),
        "ircs" | "irc+insecure" => Ok(Box::new(TcpConnector::new(url, tls)?)),
        "irc" => Err(TransportError::InvalidUrl(
            "use ircs:// for TLS or irc+insecure:// for plain text".into(),
        )),
        _ => Err(TransportError::InvalidUrl(format!(
            "unsupported URL {url:?}; expected wss://, ws://, ircs:// or irc+insecure://"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_transport_by_scheme() {
        for url in [
            "wss://irc.example.com/socket",
            "ws://127.0.0.1:8080/",
            "ircs://irc.example.com",
            "irc+insecure://127.0.0.1:6667",
        ] {
            assert!(
                connector_for_url(url, TlsOptions::default()).is_ok(),
                "{url}"
            );
        }
        for url in ["irc://irc.example.com", "https://example.com", "nonsense"] {
            assert!(
                connector_for_url(url, TlsOptions::default()).is_err(),
                "{url}"
            );
        }
    }
}
