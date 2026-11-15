//! One connection's task: connect, register, pass traffic to the
//! coordinator, keep the connection alive, and reconnect with backoff.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

use super::{Backoff, ConnectionState};
use crate::auth::CredentialProvider;
use crate::client::{Client, ClientError, ConnectOptions, Incoming};
use crate::model::NetworkId;
use crate::proto::Message;
use crate::sasl::SaslCredentials;
use crate::session::{Registered, RegistrationError};
use crate::transport::Connector;

/// `None` for the control connection, else the bound network.
pub(super) type ConnId = Option<NetworkId>;

#[derive(Debug)]
pub(super) enum ToConn {
    Send(Message),
    Stop,
}

#[derive(Debug)]
pub(super) enum FromConn {
    State(ConnectionState),
    Registered(Box<Registered>),
    Incoming(Incoming),
}

pub(super) type FromConnTx = mpsc::UnboundedSender<(ConnId, FromConn)>;

/// How a connection logs in.
pub(super) enum Login {
    /// soju: credentials fetched (and refreshed) for each attempt; SASL
    /// required.
    Provider(Arc<dyn CredentialProvider>),
    /// A network without a bouncer: SASL if configured.
    Sasl(Option<SaslCredentials>),
}

/// Everything one connection task needs.
pub(super) struct ConnSpec {
    pub connector: Arc<dyn Connector>,
    pub login: Login,
    pub opts: ConnectOptions,
    pub backoff: Backoff,
    pub keepalive: Duration,
}

impl ConnSpec {
    /// A connection to soju: the control connection (`id` is `None`), or
    /// one bound to network `id`.
    pub(super) fn bouncer(
        connector: Arc<dyn Connector>,
        credentials: Arc<dyn CredentialProvider>,
        id: &ConnId,
        config: &super::SessionConfig,
    ) -> Self {
        let mut opts = ConnectOptions::new(config.nick.clone());
        opts.bind_network = id.clone();
        if id.is_none() {
            // Live network updates on the control connection.
            opts.wanted_caps
                .push("soju.im/bouncer-networks-notify".into());
        }
        ConnSpec {
            connector,
            login: Login::Provider(credentials),
            opts,
            backoff: config.backoff,
            keepalive: config.keepalive,
        }
    }
}

pub(super) fn spawn(
    id: ConnId,
    spec: Arc<ConnSpec>,
    events: FromConnTx,
) -> mpsc::UnboundedSender<ToConn> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(id, spec, rx, events));
    tx
}

enum Ended {
    Stop,
    Error(String),
}

async fn run(
    id: ConnId,
    spec: Arc<ConnSpec>,
    mut rx: mpsc::UnboundedReceiver<ToConn>,
    events: FromConnTx,
) {
    let emit = |ev: FromConn| {
        let _ = events.send((id.clone(), ev));
    };
    let mut attempt = 0u32;

    loop {
        emit(FromConn::State(ConnectionState::Connecting));
        let opts = spec.opts.clone();
        let connect = async {
            match &spec.login {
                Login::Provider(credentials) => {
                    Client::connect(spec.connector.as_ref(), credentials.as_ref(), opts).await
                }
                Login::Sasl(sasl) => {
                    Client::connect_with(spec.connector.as_ref(), sasl.clone(), opts).await
                }
            }
        };

        let connected = tokio::select! {
            r = connect => r,
            () = until_stopped(&mut rx) => return,
        };

        let error = match connected {
            Ok(mut client) => {
                attempt = 0;
                emit(FromConn::Registered(Box::new(client.info().clone())));
                emit(FromConn::State(ConnectionState::Connected));
                match serve(&mut client, &mut rx, &emit, spec.keepalive).await {
                    Ended::Stop => {
                        client.quit().await.ok();
                        return;
                    }
                    Ended::Error(e) => e,
                }
            }
            Err(ClientError::Registration(RegistrationError::SaslFailed { line, .. })) => {
                emit(FromConn::State(ConnectionState::AuthFailed {
                    reason: line,
                }));
                return;
            }
            // Wrong kind of login for this server (e.g. single sign-on
            // against a soju with its own accounts): retrying can't help.
            Err(ClientError::Registration(RegistrationError::SaslUnavailable {
                wanted,
                offered,
            })) => {
                let offered = match offered {
                    Some(o) if !o.is_empty() => format!("it offers {o}"),
                    _ => "it offers no SASL login".into(),
                };
                emit(FromConn::State(ConnectionState::AuthFailed {
                    reason: format!("the server doesn't accept {wanted} logins ({offered})"),
                }));
                return;
            }
            Err(ClientError::Auth(e)) => {
                emit(FromConn::State(ConnectionState::AuthFailed {
                    reason: e.to_string(),
                }));
                return;
            }
            Err(e) => e.to_string(),
        };

        let delay = spec.backoff.delay(attempt);
        attempt = attempt.saturating_add(1);
        tracing::debug!(network = ?id, %error, ?delay, "connection lost");
        emit(FromConn::State(ConnectionState::Disconnected {
            error,
            retry_in: delay,
        }));
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = until_stopped(&mut rx) => return,
        }
    }
}

/// Resolves when told to stop (or the coordinator is gone). Sends arriving
/// while not connected are dropped; the coordinator doesn't forward any
/// while a connection is down.
async fn until_stopped(rx: &mut mpsc::UnboundedReceiver<ToConn>) {
    loop {
        match rx.recv().await {
            None | Some(ToConn::Stop) => return,
            Some(ToConn::Send(msg)) => {
                tracing::debug!(command = %msg.command, "dropping send while not connected");
            }
        }
    }
}

async fn serve(
    client: &mut Client,
    rx: &mut mpsc::UnboundedReceiver<ToConn>,
    emit: &impl Fn(FromConn),
    keepalive: Duration,
) -> Ended {
    let mut last_traffic = Instant::now();
    let mut pinged = false;
    let mut tick = tokio::time::interval(keepalive / 4);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(ToConn::Send(msg)) => {
                    if let Err(e) = client.send(&msg).await {
                        return Ended::Error(e.to_string());
                    }
                }
                Some(ToConn::Stop) | None => return Ended::Stop,
            },
            incoming = client.next_incoming() => {
                let incoming = match incoming {
                    Ok(i) => i,
                    Err(e) => return Ended::Error(e.to_string()),
                };
                last_traffic = Instant::now();
                pinged = false;
                if let Incoming::Message(msg) = &incoming {
                    match msg.command.as_str() {
                        "PING" => {
                            let pong = Message::new("PONG", msg.params.clone());
                            if let Err(e) = client.send(&pong).await {
                                return Ended::Error(e.to_string());
                            }
                            continue;
                        }
                        "PONG" => continue,
                        "ERROR" => {
                            return Ended::Error(format!(
                                "server closed the connection: {}",
                                msg.param(0).unwrap_or("")
                            ));
                        }
                        _ => {}
                    }
                }
                emit(FromConn::Incoming(incoming));
            },
            _ = tick.tick() => {
                let idle = last_traffic.elapsed();
                if idle >= keepalive * 2 {
                    return Ended::Error("no response from server".into());
                }
                if idle >= keepalive && !pinged {
                    pinged = true;
                    if let Err(e) = client.send(&Message::new("PING", ["backroom"])).await {
                        return Ended::Error(e.to_string());
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use secrecy::SecretString;

    use super::*;
    use crate::auth::StaticCredentials;
    use crate::live::{Backoff, SessionConfig};
    use crate::sasl::SaslCredentials;
    use crate::transport::mock::{MockConnector, Step, expect, server};
    use crate::transport::{Connector, Transport, TransportError};

    /// Fails the first `failures` connects, then hands out `then`.
    struct Flaky {
        failures: u32,
        attempts: AtomicU32,
        then: MockConnector,
    }

    #[async_trait]
    impl Connector for Flaky {
        async fn connect(&self) -> Result<Box<dyn Transport>, TransportError> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) < self.failures {
                return Err(TransportError::Io(std::io::Error::other("refused")));
            }
            self.then.connect().await
        }
    }

    /// A soju connection: `id` `None` for the control connection.
    fn spec(connector: Arc<dyn Connector>, id: &ConnId) -> Arc<ConnSpec> {
        let creds = Arc::new(StaticCredentials(SaslCredentials::Plain {
            username: "dave".into(),
            password: SecretString::from("pw"),
        }));
        let mut c = SessionConfig::new(connector.clone(), creds.clone(), "dave");
        c.backoff = Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(8),
        };
        Arc::new(ConnSpec::bouncer(connector, creds, id, &c))
    }

    fn registration(fail_sasl: bool) -> Vec<Step> {
        let mut s = vec![
            expect("USER"),
            server(
                ":soju CAP * LS :sasl=PLAIN batch soju.im/bouncer-networks soju.im/bouncer-networks-notify",
            ),
            expect("CAP REQ"),
            server(
                ":soju CAP * ACK :sasl batch soju.im/bouncer-networks soju.im/bouncer-networks-notify",
            ),
            expect("AUTHENTICATE PLAIN"),
            server("AUTHENTICATE +"),
            expect("AUTHENTICATE "),
        ];
        if fail_sasl {
            s.push(server(":soju 904 * :Invalid credentials"));
            return s;
        }
        s.extend([
            server(":soju 903 dave :ok"),
            expect("CAP END"),
            server(":soju 001 dave :Welcome"),
            server(":soju 422 dave :No MOTD"),
        ]);
        s
    }

    async fn collect(
        rx: &mut mpsc::UnboundedReceiver<(ConnId, FromConn)>,
        until: impl Fn(&FromConn) -> bool,
    ) -> Vec<String> {
        let seen = Mutex::new(Vec::new());
        loop {
            let (_, ev) = rx.recv().await.expect("connection task ended");
            let done = until(&ev);
            seen.lock().unwrap().push(match &ev {
                FromConn::State(s) => format!("{s:?}"),
                FromConn::Registered(_) => "Registered".into(),
                FromConn::Incoming(i) => format!("{i:?}"),
            });
            if done {
                return seen.into_inner().unwrap();
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_with_backoff_then_registers() {
        let mut script = registration(false);
        script.push(server(":alice!a@h PRIVMSG dave :hi"));
        let flaky = Arc::new(Flaky {
            failures: 2,
            attempts: AtomicU32::new(0),
            then: MockConnector::new(script),
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn = spawn(None, spec(flaky.clone(), &None), tx);

        let seen = collect(&mut rx, |e| matches!(e, FromConn::Incoming(_))).await;
        assert_eq!(seen[0], "Connecting");
        assert!(seen[1].contains("retry_in: 1s"), "{seen:?}");
        assert_eq!(seen[2], "Connecting");
        assert!(seen[3].contains("retry_in: 2s"), "{seen:?}");
        assert_eq!(&seen[4..7], ["Connecting", "Registered", "Connected"]);
        assert!(seen[7].contains("PRIVMSG"));
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 3);

        // Control connection asked for live network updates.
        let sent = flaky.then.sent();
        let req = sent.iter().find(|l| l.starts_with("CAP REQ")).unwrap();
        assert!(req.contains("soju.im/bouncer-networks-notify"), "{req}");
        conn.send(ToConn::Stop).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_login_stops_reconnecting() {
        let mock = MockConnector::new(registration(true));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _conn = spawn(None, spec(Arc::new(mock), &None), tx);
        let seen = collect(&mut rx, |e| {
            matches!(e, FromConn::State(ConnectionState::AuthFailed { .. }))
        })
        .await;
        assert_eq!(seen[0], "Connecting");
        assert!(seen[1].contains("Invalid credentials"), "{seen:?}");
        // The task has ended: no further states, even after a long wait.
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_login_method_stops_reconnecting() {
        let mock = MockConnector::new(vec![
            expect("USER"),
            server(":soju CAP * LS :sasl=OAUTHBEARER batch"),
        ]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _conn = spawn(None, spec(Arc::new(mock), &None), tx);
        let seen = collect(&mut rx, |e| {
            matches!(e, FromConn::State(ConnectionState::AuthFailed { .. }))
        })
        .await;
        assert!(
            seen[1].contains("doesn't accept PLAIN logins (it offers OAUTHBEARER)"),
            "{seen:?}"
        );
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn answers_ping_and_forwards_sends() {
        let mut script = registration(false);
        script.extend([
            server("PING :abc"),
            expect("PONG abc"),
            server(":alice!a@h PRIVMSG dave :after ping"),
            expect("PRIVMSG alice hello"),
        ]);
        let mock = MockConnector::concurrent(script);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn = spawn(
            Some("1".into()),
            spec(Arc::new(mock.clone()), &Some("1".into())),
            tx,
        );
        collect(&mut rx, |e| matches!(e, FromConn::Incoming(_))).await;
        conn.send(ToConn::Send(Message::new("PRIVMSG", ["alice", "hello"])))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sent = mock.sent();
        assert!(sent.contains(&"PONG abc".to_owned()), "{sent:?}");
        assert!(sent.contains(&"PRIVMSG alice hello".to_owned()), "{sent:?}");
        assert!(sent.contains(&"BOUNCER BIND 1".to_owned()), "{sent:?}");
    }
}
