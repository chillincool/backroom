//! IRC over WebSocket (<https://ircv3.net/specs/extensions/websocket>).
//!
//! One IRC line per WebSocket message, without CRLF. We offer both
//! subprotocols, preferring `text.ircv3.net`, and reply in whichever frame
//! type the server selected.

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
use tokio_tungstenite::{Connector as WsTlsConnector, MaybeTlsStream, WebSocketStream};
use url::Url;

use super::{Connector, TlsOptions, Transport, TransportError};

const SUBPROTOCOL_TEXT: &str = "text.ircv3.net";
const SUBPROTOCOL_BINARY: &str = "binary.ircv3.net";

/// Upper bound on one incoming line: 8191 bytes of tags + 512 for the rest,
/// with headroom. Protects against unbounded frames.
const MAX_MESSAGE_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct WebSocketConnector {
    url: Url,
    tls: TlsOptions,
}

impl WebSocketConnector {
    pub fn new(url: &str) -> Result<Self, TransportError> {
        Self::with_tls(url, TlsOptions::default())
    }

    pub fn with_tls(url: &str, tls: TlsOptions) -> Result<Self, TransportError> {
        let url = Url::parse(url).map_err(|e| TransportError::InvalidUrl(e.to_string()))?;
        match url.scheme() {
            "wss" | "ws" => Ok(WebSocketConnector { url, tls }),
            other => Err(TransportError::InvalidUrl(format!(
                "unsupported scheme {other:?}, expected wss:// or ws://"
            ))),
        }
    }
}

#[async_trait]
impl Connector for WebSocketConnector {
    async fn connect(&self) -> Result<Box<dyn Transport>, TransportError> {
        let mut request = self.url.as_str().into_client_request()?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("text.ircv3.net, binary.ircv3.net"),
        );

        let tls = match self.url.scheme() {
            "wss" => Some(WsTlsConnector::Rustls(self.tls.client_config()?)),
            _ => None,
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_MESSAGE_SIZE));

        let (stream, response) =
            tokio_tungstenite::connect_async_tls_with_config(request, Some(config), false, tls)
                .await?;

        let protocol = response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|v| v.to_str().ok());
        let binary = protocol == Some(SUBPROTOCOL_BINARY);
        tracing::debug!(
            url = %self.url,
            subprotocol = protocol.unwrap_or("(none)"),
            "WebSocket connected"
        );
        if protocol.is_none() {
            tracing::warn!(
                "server did not select an IRCv3 WebSocket subprotocol; assuming {SUBPROTOCOL_TEXT}"
            );
        }

        Ok(Box::new(WebSocketTransport { stream, binary }))
    }
}

struct WebSocketTransport {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Send binary frames (server picked `binary.ircv3.net`).
    binary: bool,
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn send(&mut self, line: &str) -> Result<(), TransportError> {
        debug_assert!(!line.contains(['\r', '\n']), "IRC line contains CR/LF");
        let frame = if self.binary {
            WsMessage::Binary(line.as_bytes().to_vec().into())
        } else {
            WsMessage::Text(line.into())
        };
        self.stream.send(frame).await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            let Some(frame) = self.stream.next().await else {
                return Ok(None);
            };
            let line = match frame {
                Ok(WsMessage::Text(text)) => text.as_str().to_owned(),
                Ok(WsMessage::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
                Ok(WsMessage::Close(_)) => return Ok(None),
                // tungstenite queues the Pong itself; it goes out on the next write/flush.
                Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_)) => continue,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            };
            // Senders must not include CRLF, but be lenient.
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                continue;
            }
            return Ok(Some(line.to_owned()));
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        match self.stream.close(None).await {
            Ok(())
            | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_websocket_urls() {
        assert!(WebSocketConnector::new("wss://irc.chillincool.net/socket").is_ok());
        assert!(WebSocketConnector::new("ws://localhost:8080/socket").is_ok());
        assert!(WebSocketConnector::new("ircs://irc.example.com:6697").is_err());
        assert!(WebSocketConnector::new("not a url").is_err());
    }

    #[test]
    fn tls_config_builds() {
        TlsOptions::default().client_config().unwrap();
    }
}
