//! Async driver: owns a transport, runs registration, and offers
//! request/response helpers on top of batches.
//!
//! Raw traffic is traced at `TRACE` on target `backroom_core::raw`, with
//! `AUTHENTICATE` arguments redacted.

use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::auth::{AuthError, CredentialProvider};
use crate::batch::{Batch, BatchCollector, Feed};
use crate::bouncer::{Network, NetworkUpdate, parse_network_message};
use crate::proto::caps::WANTED_CAPS;
use crate::proto::{Message, ParseError};
use crate::session::{Outgoing, Registered, Registration, RegistrationConfig, RegistrationError};
use crate::transport::{Connector, Transport, TransportError};

const RAW: &str = "backroom_core::raw";

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub nick: String,
    pub username: String,
    pub realname: String,
    pub bind_network: Option<String>,
    pub wanted_caps: Vec<String>,
    /// Applies to registration and to each request.
    pub timeout: Duration,
}

impl ConnectOptions {
    pub fn new(nick: impl Into<String>) -> Self {
        let nick = nick.into();
        ConnectOptions {
            username: nick.clone(),
            nick,
            realname: "Backroom".into(),
            bind_network: None,
            wanted_caps: WANTED_CAPS.iter().map(|s| s.to_string()).collect(),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Registration(#[from] RegistrationError),
    #[error("cannot parse server line {line:?}: {source}")]
    Parse { line: String, source: ParseError },
    #[error("server closed the connection")]
    Closed,
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("server did not enable the {0} capability")]
    MissingCap(&'static str),
    #[error("server error: {line}")]
    Fail { line: String },
}

pub struct Client {
    transport: Box<dyn Transport>,
    info: Registered,
    batches: BatchCollector,
    timeout: Duration,
}

impl Client {
    /// Connects, negotiates capabilities, authenticates and registers.
    pub async fn connect(
        connector: &dyn Connector,
        credentials: &dyn CredentialProvider,
        opts: ConnectOptions,
    ) -> Result<Client, ClientError> {
        // Fetch (and maybe refresh) credentials before opening the socket.
        let sasl = credentials.credentials().await?;
        let mut transport = connector.connect().await?;
        let deadline = Instant::now() + opts.timeout;

        let mut reg = Registration::new(RegistrationConfig {
            nick: opts.nick,
            username: opts.username,
            realname: opts.realname,
            wanted_caps: opts.wanted_caps,
            sasl: Some(sasl),
            bind_network: opts.bind_network,
        });

        let info = loop {
            for out in reg.take_outgoing() {
                send_outgoing(transport.as_mut(), out).await?;
            }
            let line = recv_line(transport.as_mut(), deadline, "registration").await?;
            let msg = match Message::parse(&line) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "ignoring unparseable line during registration");
                    continue;
                }
            };
            if let Some(done) = reg.handle(&msg, &line)? {
                for out in reg.take_outgoing() {
                    send_outgoing(transport.as_mut(), out).await?;
                }
                break done;
            }
        };

        tracing::debug!(
            nick = %info.nick,
            account = ?info.account,
            caps = ?info.enabled_caps,
            "registered"
        );
        Ok(Client {
            transport,
            info,
            batches: BatchCollector::default(),
            timeout: opts.timeout,
        })
    }

    pub fn info(&self) -> &Registered {
        &self.info
    }

    pub async fn send(&mut self, msg: &Message) -> Result<(), ClientError> {
        send_message(self.transport.as_mut(), msg).await
    }

    /// Lists upstream networks (`BOUNCER LISTNETWORKS`).
    pub async fn list_networks(&mut self) -> Result<Vec<Network>, ClientError> {
        self.require_cap("soju.im/bouncer-networks")?;
        self.send(&Message::new("BOUNCER", ["LISTNETWORKS"]))
            .await?;
        let batch = self
            .wait_for_batch(
                "network list",
                |b| b.kind == "soju.im/bouncer-networks",
                |m| m.command == "FAIL" && m.param(0) == Some("BOUNCER"),
            )
            .await?;
        Ok(batch
            .messages
            .iter()
            .filter_map(parse_network_message)
            .filter_map(|u| match u {
                NetworkUpdate::Upsert(n) => Some(n),
                NetworkUpdate::Removed { .. } => None,
            })
            .collect())
    }

    /// Fetches the latest `limit` messages for `target` in chronological
    /// order (`CHATHISTORY LATEST <target> * <limit>`). The connection must
    /// be bound to a network.
    pub async fn chathistory_latest(
        &mut self,
        target: &str,
        limit: usize,
    ) -> Result<Vec<Message>, ClientError> {
        self.require_cap("draft/chathistory")?;
        let max = self
            .info
            .isupport
            .get("CHATHISTORY")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0);
        let limit = max.map_or(limit, |m| limit.min(m));

        self.send(&Message::new(
            "CHATHISTORY",
            [
                "LATEST".to_owned(),
                target.to_owned(),
                "*".into(),
                limit.to_string(),
            ],
        ))
        .await?;
        let wanted = target.to_ascii_lowercase();
        let batch = self
            .wait_for_batch(
                "chat history",
                |b| {
                    b.kind == "chathistory"
                        && b.params.first().map(|t| t.to_ascii_lowercase()) == Some(wanted.clone())
                },
                |m| m.command == "FAIL" && m.param(0) == Some("CHATHISTORY"),
            )
            .await?;
        Ok(batch.messages)
    }

    /// Reads until an unbatched message matches `pred`, answering PINGs.
    /// Other traffic is discarded.
    pub async fn wait_for(
        &mut self,
        what: &'static str,
        pred: impl Fn(&Message) -> bool,
    ) -> Result<Message, ClientError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let line = recv_line(self.transport.as_mut(), deadline, what).await?;
            let msg = Message::parse(&line).map_err(|source| ClientError::Parse {
                line: line.clone(),
                source,
            })?;
            if msg.command == "PING" {
                self.send(&Message::new("PONG", msg.params.clone())).await?;
                continue;
            }
            if let Feed::Unbatched(msg) = self.batches.feed(msg) {
                if pred(&msg) {
                    return Ok(msg);
                }
                if msg.command == "ERROR" {
                    return Err(ClientError::Fail { line });
                }
            }
        }
    }

    pub async fn quit(mut self) -> Result<(), ClientError> {
        self.send(&Message::new("QUIT", ["Backroom"])).await?;
        self.transport.close().await?;
        Ok(())
    }

    fn require_cap(&self, cap: &'static str) -> Result<(), ClientError> {
        if self.info.has_cap(cap) {
            Ok(())
        } else {
            Err(ClientError::MissingCap(cap))
        }
    }

    /// Reads until a completed batch matches `is_reply`, or a line matches
    /// `is_error`. Unrelated traffic is discarded.
    async fn wait_for_batch(
        &mut self,
        what: &'static str,
        is_reply: impl Fn(&Batch) -> bool,
        is_error: impl Fn(&Message) -> bool,
    ) -> Result<Batch, ClientError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let line = recv_line(self.transport.as_mut(), deadline, what).await?;
            let msg = Message::parse(&line).map_err(|source| ClientError::Parse {
                line: line.clone(),
                source,
            })?;
            if msg.command == "PING" {
                self.send(&Message::new("PONG", msg.params.clone())).await?;
                continue;
            }
            if is_error(&msg) {
                return Err(ClientError::Fail { line });
            }
            match self.batches.feed(msg) {
                Feed::Completed(batch) if is_reply(&batch) => return Ok(batch),
                Feed::Completed(batch) => {
                    tracing::debug!(kind = %batch.kind, "ignoring unrelated batch");
                }
                Feed::Unbatched(msg) => {
                    if msg.command == "ERROR" {
                        return Err(ClientError::Fail { line });
                    }
                }
                Feed::Buffered => {}
            }
        }
    }
}

async fn recv_line(
    transport: &mut dyn Transport,
    deadline: Instant,
    what: &'static str,
) -> Result<String, ClientError> {
    match tokio::time::timeout_at(deadline, transport.recv()).await {
        Err(_) => Err(ClientError::Timeout(what)),
        Ok(Err(e)) => Err(e.into()),
        Ok(Ok(None)) => Err(ClientError::Closed),
        Ok(Ok(Some(line))) => {
            tracing::trace!(target: RAW, "<- {line}");
            Ok(line)
        }
    }
}

async fn send_message(transport: &mut dyn Transport, msg: &Message) -> Result<(), ClientError> {
    let line = msg.to_string();
    tracing::trace!(target: RAW, "-> {line}");
    transport.send(&line).await?;
    Ok(())
}

async fn send_outgoing(transport: &mut dyn Transport, out: Outgoing) -> Result<(), ClientError> {
    match out {
        Outgoing::Message(msg) => send_message(transport, &msg).await,
        Outgoing::Authenticate(arg) => send_authenticate(transport, &arg).await,
    }
}

async fn send_authenticate(
    transport: &mut dyn Transport,
    arg: &SecretString,
) -> Result<(), ClientError> {
    tracing::trace!(target: RAW, "-> AUTHENTICATE <redacted>");
    let line = Zeroizing::new(format!("AUTHENTICATE {}", arg.expose_secret()));
    transport.send(&line).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::StaticCredentials;
    use crate::sasl::SaslCredentials;
    use crate::transport::mock::{MockConnector, expect, server};

    fn plain() -> StaticCredentials {
        StaticCredentials(SaslCredentials::Plain {
            username: "chris".into(),
            password: SecretString::from("pw"),
        })
    }

    fn registration_script(bind: bool) -> Vec<crate::transport::mock::Step> {
        let mut s = vec![
            expect("USER"),
            server(
                ":soju CAP * LS :sasl=PLAIN,OAUTHBEARER batch server-time draft/chathistory soju.im/bouncer-networks",
            ),
            expect("CAP REQ"),
            server(
                ":soju CAP * ACK :sasl batch server-time draft/chathistory soju.im/bouncer-networks",
            ),
            expect("AUTHENTICATE PLAIN"),
            server("AUTHENTICATE +"),
            expect("AUTHENTICATE "),
            server(":soju 900 chris chris!chris@soju chris :You are now logged in as chris"),
            server(":soju 903 chris :SASL authentication successful"),
        ];
        if bind {
            s.push(expect("BOUNCER BIND"));
        }
        s.extend([
            expect("CAP END"),
            server(":soju 001 chris :Welcome"),
            server(":soju 005 chris CHATHISTORY=100 :are supported"),
            server(":soju 422 chris :No MOTD"),
        ]);
        s
    }

    #[tokio::test]
    async fn lists_networks() {
        let mut script = registration_script(false);
        script.extend([
            expect("BOUNCER LISTNETWORKS"),
            server(":soju NOTICE chris :unrelated"),
            server("PING :x"),
            expect("PONG"),
            server("BATCH +n soju.im/bouncer-networks"),
            server("@batch=n BOUNCER NETWORK 1 name=Libera;state=connected"),
            server("@batch=n BOUNCER NETWORK 2 name=OFTC;state=disconnected"),
            server("BATCH -n"),
        ]);
        let conn = MockConnector::new(script);
        let mut client = Client::connect(&conn, &plain(), ConnectOptions::new("chris"))
            .await
            .unwrap();
        assert_eq!(client.info().account.as_deref(), Some("chris"));
        let nets = client.list_networks().await.unwrap();
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[1].name(), Some("OFTC"));
        assert!(conn.sent().contains(&"PONG x".to_owned()));
    }

    #[tokio::test]
    async fn fetches_history_on_bound_connection() {
        let mut script = registration_script(true);
        script.extend([
            expect("CHATHISTORY LATEST #Chan * 100"),
            server(":n JOIN #chan"),
            server("BATCH +h chathistory #chan"),
            server("@batch=h;time=2026-10-23T10:00:00.000Z :a!a@h PRIVMSG #chan :first"),
            server("@batch=h;time=2026-10-23T10:01:00.000Z :b!b@h PRIVMSG #chan :second"),
            server("BATCH -h"),
        ]);
        let conn = MockConnector::new(script);
        let mut opts = ConnectOptions::new("chris");
        opts.bind_network = Some("1".into());
        let mut client = Client::connect(&conn, &plain(), opts).await.unwrap();
        // Limit is clamped to ISUPPORT CHATHISTORY=100.
        let msgs = client.chathistory_latest("#Chan", 500).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].param(1), Some("first"));
        assert!(conn.sent().contains(&"BOUNCER BIND 1".to_owned()));
    }

    #[tokio::test]
    async fn chathistory_fail_is_reported() {
        let mut script = registration_script(false);
        script.extend([
            expect("CHATHISTORY"),
            server(
                ":soju FAIL CHATHISTORY INVALID_TARGET LATEST #x :Messages could not be retrieved",
            ),
        ]);
        let conn = MockConnector::new(script);
        let mut client = Client::connect(&conn, &plain(), ConnectOptions::new("chris"))
            .await
            .unwrap();
        let err = client.chathistory_latest("#x", 10).await.unwrap_err();
        assert!(matches!(err, ClientError::Fail { line } if line.contains("INVALID_TARGET")));
    }

    #[tokio::test]
    async fn closed_connection_during_registration() {
        let conn = MockConnector::new([expect("USER")]);
        let err = Client::connect(&conn, &plain(), ConnectOptions::new("chris"))
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ClientError::Closed));
    }
}
