//! One-shot loopback HTTP listener for the OAuth redirect (RFC 8252 §7.3).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const CALLBACK_PATH: &str = "/callback";

pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    /// Binds before the browser is opened so the redirect can't race us.
    pub async fn bind(port: u16) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| {
                format!("cannot listen on 127.0.0.1:{port} (in use? try --redirect-port)")
            })?;
        Ok(Loopback { listener, port })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Waits for `GET /callback?...` and returns the full callback URL.
    /// Other requests (e.g. `/favicon.ico`) get a 404 and are ignored.
    pub async fn wait_for_callback(&self) -> Result<String> {
        loop {
            let (mut sock, _) = self.listener.accept().await?;
            match tokio::time::timeout(Duration::from_secs(10), read_request_target(&mut sock))
                .await
            {
                Ok(Ok(target)) if target.split('?').next() == Some(CALLBACK_PATH) => {
                    respond(&mut sock, "200 OK", PAGE_DONE).await;
                    return Ok(format!("http://127.0.0.1:{}{target}", self.port));
                }
                Ok(Ok(_)) => respond(&mut sock, "404 Not Found", "Not found").await,
                Ok(Err(e)) => tracing::debug!("ignoring bad loopback request: {e:#}"),
                Err(_) => tracing::debug!("loopback request timed out"),
            }
        }
    }
}

/// Reads the request head and returns the request target. The target holds
/// the authorization code, so it is never logged.
async fn read_request_target(sock: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            bail!("connection closed before request was complete");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 16 * 1024 {
            bail!("request head too large");
        }
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&buf)? {
            httparse::Status::Complete(_) => {
                if req.method != Some("GET") {
                    bail!("unexpected method {:?}", req.method);
                }
                return Ok(req.path.unwrap_or("/").to_owned());
            }
            httparse::Status::Partial => continue,
        }
    }
}

async fn respond(sock: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(response.as_bytes()).await;
    let _ = sock.shutdown().await;
}

const PAGE_DONE: &str = "<!doctype html><meta charset=utf-8><title>Backroom</title>\
<body style=\"font-family:system-ui;margin:3em\"><h1>Backroom</h1>\
<p>Login response received. You can close this tab and return to the terminal.</p>";
