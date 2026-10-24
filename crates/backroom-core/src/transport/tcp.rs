//! IRC over TCP, with or without TLS, using soju's URL schemes:
//! `ircs://host[:port]` (TLS, default 6697) and `irc+insecure://host[:port]`
//! (plain text, default 6667). Lines end in CRLF.

use async_trait::async_trait;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use url::Url;

use super::{Connector, TlsOptions, Transport, TransportError};

/// Same bound as the WebSocket transport: 8191 bytes of tags + 512, with headroom.
const MAX_LINE: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct TcpConnector {
    host: String,
    port: u16,
    tls: Option<TlsOptions>,
}

impl TcpConnector {
    pub fn new(url: &str, tls: TlsOptions) -> Result<Self, TransportError> {
        let url = Url::parse(url).map_err(|e| TransportError::InvalidUrl(e.to_string()))?;
        let (default_port, tls) = match url.scheme() {
            "ircs" => (6697, Some(tls)),
            "irc+insecure" => (6667, None),
            other => {
                return Err(TransportError::InvalidUrl(format!(
                    "unsupported scheme {other:?}, expected ircs:// or irc+insecure://"
                )));
            }
        };
        let host = match url.host() {
            // Bracketed IPv6 literals come back without the brackets here.
            Some(url::Host::Ipv6(addr)) => addr.to_string(),
            Some(host) => host.to_string(),
            None => return Err(TransportError::InvalidUrl("URL has no host".into())),
        };
        Ok(TcpConnector {
            host,
            port: url.port().unwrap_or(default_port),
            tls,
        })
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

#[async_trait]
impl Connector for TcpConnector {
    async fn connect(&self) -> Result<Box<dyn Transport>, TransportError> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port)).await?;
        tcp.set_nodelay(true)?;
        let stream: Box<dyn Stream> = match &self.tls {
            None => Box::new(tcp),
            Some(options) => {
                let name = ServerName::try_from(self.host.clone())
                    .map_err(|e| TransportError::Tls(format!("bad server name: {e}")))?;
                let tls = TlsConnector::from(options.client_config()?)
                    .connect(name, tcp)
                    .await
                    .map_err(|e| TransportError::Tls(e.to_string()))?;
                Box::new(tls)
            }
        };
        tracing::debug!(host = %self.host, port = self.port, tls = self.tls.is_some(), "TCP connected");
        Ok(Box::new(TcpTransport {
            stream,
            buf: Vec::with_capacity(1024),
        }))
    }
}

struct TcpTransport {
    stream: Box<dyn Stream>,
    /// Bytes read but not yet returned as lines.
    buf: Vec<u8>,
}

#[async_trait]
impl Transport for TcpTransport {
    async fn send(&mut self, line: &str) -> Result<(), TransportError> {
        debug_assert!(!line.contains(['\r', '\n']), "IRC line contains CR/LF");
        let mut bytes = Vec::with_capacity(line.len() + 2);
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = self.buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw);
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    continue;
                }
                return Ok(Some(line.to_owned()));
            }
            if self.buf.len() > MAX_LINE {
                return Err(TransportError::LineTooLong);
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                // A final line without CRLF is still a line.
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
                return Ok(Some(line));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        match self.stream.shutdown().await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn parses_urls_and_default_ports() {
        let c = TcpConnector::new("ircs://irc.example.com", TlsOptions::default()).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.tls.is_some()),
            ("irc.example.com", 6697, true)
        );
        let c = TcpConnector::new("irc+insecure://127.0.0.1:7000", TlsOptions::default()).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.tls.is_some()),
            ("127.0.0.1", 7000, false)
        );
        let c = TcpConnector::new("ircs://[::1]:6697", TlsOptions::default()).unwrap();
        assert_eq!(c.host, "::1");
        // Plain `irc://` is refused so plaintext is always explicit.
        assert!(TcpConnector::new("irc://irc.example.com", TlsOptions::default()).is_err());
        assert!(TcpConnector::new("wss://irc.example.com", TlsOptions::default()).is_err());
    }

    /// A one-connection server that writes `script` and returns what it read.
    async fn serve(script: &'static [&'static [u8]]) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            for chunk in script {
                sock.write_all(chunk).await.unwrap();
                sock.flush().await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            let mut received = Vec::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                sock.read_to_end(&mut received),
            )
            .await;
            received
        });
        (format!("irc+insecure://{addr}"), handle)
    }

    #[tokio::test]
    async fn frames_lines_split_across_reads() {
        let (url, server) = serve(&[
            b"PING :a\r\nNOTI",
            b"CE x :split\r\n\r\n",
            b"LF :only\n",
            b"TAIL",
        ])
        .await;
        let mut t = TcpConnector::new(&url, TlsOptions::default())
            .unwrap()
            .connect()
            .await
            .unwrap();
        t.send("PONG :a").await.unwrap();
        assert_eq!(t.recv().await.unwrap().as_deref(), Some("PING :a"));
        assert_eq!(t.recv().await.unwrap().as_deref(), Some("NOTICE x :split"));
        assert_eq!(t.recv().await.unwrap().as_deref(), Some("LF :only"));
        assert_eq!(t.recv().await.unwrap().as_deref(), Some("TAIL"));
        assert_eq!(t.recv().await.unwrap(), None);
        assert_eq!(server.await.unwrap(), b"PONG :a\r\n");
    }

    #[tokio::test]
    async fn rejects_overlong_lines() {
        static LONG: [u8; 20 * 1024] = [b'a'; 20 * 1024];
        static SCRIPT: [&[u8]; 1] = [&LONG];
        let (url, _server) = serve(&SCRIPT).await;
        let mut t = TcpConnector::new(&url, TlsOptions::default())
            .unwrap()
            .connect()
            .await
            .unwrap();
        assert!(matches!(t.recv().await, Err(TransportError::LineTooLong)));
    }
}
