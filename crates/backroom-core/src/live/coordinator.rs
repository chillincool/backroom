//! Owns the session state and routes between the front end and connections.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use super::connection::{self, ConnId, FromConn, FromConnTx, ToConn};
use super::{Command, ConnectionState, Event, SessionConfig};
use crate::batch::Batch;
use crate::bouncer::{NetworkUpdate, parse_network_message};
use crate::client::Incoming;
use crate::model::{
    BufferKind, ChatMessage, NetworkId, NetworkInfo, RouteContext, SERVER_BUFFER, buffer_key,
    irc_timestamp, route,
};
use crate::proto::Message;
use crate::session::Registered;

/// Starts a connection task; replaced in tests.
type Spawner = Box<dyn FnMut(ConnId, FromConnTx) -> mpsc::UnboundedSender<ToConn> + Send>;

pub(super) async fn run(
    config: Arc<SessionConfig>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
) {
    let spawner: Spawner = Box::new(move |id, tx| connection::spawn(id, config.clone(), tx));
    let (from_conn_tx, mut from_conn) = mpsc::unbounded_channel();
    let mut c = Coordinator::new(spawner, from_conn_tx, events);
    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                None | Some(Command::Shutdown) => break,
                Some(cmd) => c.command(cmd),
            },
            Some((id, ev)) = from_conn.recv() => c.on_connection(id, ev),
        }
    }
    c.shutdown();
}

struct Network {
    info: NetworkInfo,
    conn: mpsc::UnboundedSender<ToConn>,
    connected: bool,
    nick: String,
    chantypes: String,
    caps: BTreeSet<String>,
    /// Server limit on CHATHISTORY page size, if any.
    history_max: Option<usize>,
    /// Keyed by [`buffer_key`].
    buffers: HashMap<String, BufferKind>,
}

impl Network {
    fn ctx(&self) -> RouteContext<'_> {
        RouteContext {
            own_nick: &self.nick,
            chantypes: &self.chantypes,
        }
    }
}

pub(super) struct Coordinator {
    spawn: Spawner,
    from_conn_tx: FromConnTx,
    events: mpsc::UnboundedSender<Event>,
    control: mpsc::UnboundedSender<ToConn>,
    networks: BTreeMap<NetworkId, Network>,
}

impl Coordinator {
    fn new(
        mut spawn: Spawner,
        from_conn_tx: FromConnTx,
        events: mpsc::UnboundedSender<Event>,
    ) -> Self {
        let control = spawn(None, from_conn_tx.clone());
        Coordinator {
            spawn,
            from_conn_tx,
            events,
            control,
            networks: BTreeMap::new(),
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn error(&self, network: Option<&str>, message: impl Into<String>) {
        self.emit(Event::Error {
            network: network.map(str::to_owned),
            message: message.into(),
        });
    }

    fn shutdown(&mut self) {
        let _ = self.control.send(ToConn::Stop);
        for net in self.networks.values() {
            let _ = net.conn.send(ToConn::Stop);
        }
        self.emit(Event::Stopped);
    }

    // -- commands from the front end ----------------------------------------

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::SendMessage {
                network,
                target,
                text,
            } => {
                if text.is_empty() || text.contains(['\r', '\n', '\0']) {
                    self.error(Some(&network), "messages must be one non-empty line");
                    return;
                }
                let msg = Message::new("PRIVMSG", [target.clone(), text]);
                let Some(net) = self.send_to(&network, msg.clone()) else {
                    return;
                };
                // Without echo-message the server won't send it back, so
                // show it now.
                if !net.caps.contains("echo-message") {
                    let mut own = msg;
                    own.source = Some(format!("{}!user@host", net.nick));
                    self.live(&network, &own);
                }
            }
            Command::Join { network, channel } => {
                self.send_to(&network, Message::new("JOIN", [channel]));
            }
            Command::Part {
                network,
                channel,
                reason,
            } => {
                let mut params = vec![channel];
                params.extend(reason);
                self.send_to(&network, Message::new("PART", params));
            }
            Command::LoadHistory {
                network,
                target,
                before,
                limit,
            } => {
                let Some(net) = self.networks.get(&network) else {
                    return self.error(Some(&network), "unknown network");
                };
                if !net.caps.contains("draft/chathistory") {
                    return self.error(Some(&network), "server doesn't support chat history");
                }
                let limit = net.history_max.map_or(limit, |m| limit.min(m)).max(1);
                let msg = match before {
                    Some(t) => Message::new(
                        "CHATHISTORY",
                        [
                            "BEFORE".into(),
                            target,
                            format!("timestamp={}", irc_timestamp(t)),
                            limit.to_string(),
                        ],
                    ),
                    None => Message::new(
                        "CHATHISTORY",
                        ["LATEST".into(), target, "*".into(), limit.to_string()],
                    ),
                };
                self.send_to(&network, msg);
            }
            Command::MarkRead {
                network,
                target,
                time,
            } => {
                let Some(net) = self.networks.get(&network) else {
                    return self.error(Some(&network), "unknown network");
                };
                if !net.caps.contains("draft/read-marker") {
                    return self.error(Some(&network), "server doesn't support read markers");
                }
                self.send_to(
                    &network,
                    Message::new(
                        "MARKREAD",
                        [target, format!("timestamp={}", irc_timestamp(time))],
                    ),
                );
            }
            Command::Shutdown => {}
        }
    }

    /// Forwards to a connected network; reports an error otherwise.
    fn send_to(&self, network: &str, msg: Message) -> Option<&Network> {
        match self.networks.get(network) {
            Some(net) if net.connected => {
                let _ = net.conn.send(ToConn::Send(msg));
                Some(net)
            }
            Some(_) => {
                self.error(
                    Some(network),
                    format!("not connected; {} not sent", msg.command),
                );
                None
            }
            None => {
                self.error(Some(network), "unknown network");
                None
            }
        }
    }

    // -- traffic from connections ---------------------------------------------

    fn on_connection(&mut self, id: ConnId, ev: FromConn) {
        match (id, ev) {
            (None, FromConn::State(state)) => self.emit(Event::Connection {
                network: None,
                state,
            }),
            (None, FromConn::Registered(_)) => {}
            (None, FromConn::Incoming(Incoming::Batch(batch))) => {
                for msg in &batch.messages {
                    self.network_update(msg);
                }
            }
            (None, FromConn::Incoming(Incoming::Message(msg))) => self.network_update(&msg),

            (Some(id), FromConn::State(state)) => {
                let Some(net) = self.networks.get_mut(&id) else {
                    return;
                };
                net.connected = state == ConnectionState::Connected;
                self.emit(Event::Connection {
                    network: Some(id),
                    state,
                });
            }
            (Some(id), FromConn::Registered(reg)) => self.registered(&id, *reg),
            (Some(id), FromConn::Incoming(Incoming::Batch(batch))) => self.batch(&id, batch),
            (Some(id), FromConn::Incoming(Incoming::Message(msg))) => self.message(&id, &msg),
        }
    }

    /// `BOUNCER NETWORK` from the control connection.
    fn network_update(&mut self, msg: &Message) {
        match parse_network_message(msg) {
            Some(NetworkUpdate::Upsert(update)) => {
                let info = match self.networks.get_mut(&update.id) {
                    Some(net) => {
                        // Notifications carry only changed attributes; an
                        // empty value means the attribute was removed.
                        for (k, v) in update.attributes {
                            if v.is_empty() {
                                net.info.attributes.remove(&k);
                            } else {
                                net.info.attributes.insert(k, v);
                            }
                        }
                        net.info.clone()
                    }
                    None => {
                        let info = NetworkInfo {
                            id: update.id.clone(),
                            attributes: update.attributes,
                        };
                        let conn = (self.spawn)(Some(update.id.clone()), self.from_conn_tx.clone());
                        self.networks.insert(
                            update.id,
                            Network {
                                info: info.clone(),
                                conn,
                                connected: false,
                                nick: String::new(),
                                chantypes: "#&".into(),
                                caps: BTreeSet::new(),
                                history_max: None,
                                buffers: HashMap::new(),
                            },
                        );
                        info
                    }
                };
                self.emit(Event::NetworkUpdated(info));
            }
            Some(NetworkUpdate::Removed { id }) => {
                if let Some(net) = self.networks.remove(&id) {
                    let _ = net.conn.send(ToConn::Stop);
                    self.emit(Event::NetworkRemoved(id));
                }
            }
            None => {}
        }
    }

    fn registered(&mut self, id: &str, reg: Registered) {
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        net.nick = reg.nick;
        net.caps = reg.enabled_caps;
        if let Some(chantypes) = reg.isupport.get("CHANTYPES").filter(|c| !c.is_empty()) {
            net.chantypes = chantypes.clone();
        }
        net.history_max = reg
            .isupport
            .get("CHATHISTORY")
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0);
        if net.caps.contains("draft/chathistory") {
            // Every conversation with history, so queries show up too.
            let msg = Message::new(
                "CHATHISTORY",
                [
                    "TARGETS".into(),
                    format!("timestamp={}", irc_timestamp(Utc::now())),
                    "timestamp=1970-01-01T00:00:00.000Z".into(),
                    "1000".into(),
                ],
            );
            let _ = net.conn.send(ToConn::Send(msg));
        }
        // Every network has a server buffer, even before the server says
        // anything, so front ends always have somewhere to type commands.
        self.ensure_buffer(id, SERVER_BUFFER, BufferKind::Server);
    }

    fn batch(&mut self, id: &str, batch: Batch) {
        match batch.kind.as_str() {
            "draft/chathistory-targets" => {
                for msg in &batch.messages {
                    // CHATHISTORY TARGETS <target> <latest timestamp>
                    if msg.command == "CHATHISTORY"
                        && msg.param(0) == Some("TARGETS")
                        && let Some(target) = msg.param(1)
                    {
                        let kind = match self.networks.get(id) {
                            Some(net) if net.ctx().is_channel(target) => BufferKind::Channel,
                            _ => BufferKind::Query,
                        };
                        self.ensure_buffer(id, target, kind);
                    }
                }
            }
            "chathistory" => {
                let Some(net) = self.networks.get(id) else {
                    return;
                };
                let Some(target) = batch.params.first() else {
                    return;
                };
                let ctx = net.ctx();
                let messages: Vec<ChatMessage> = batch
                    .messages
                    .iter()
                    .filter_map(|m| route(m, &ctx).map(|(_, _, chat)| chat))
                    .collect();
                let buffer = target.clone();
                let kind = if ctx.is_channel(&buffer) {
                    BufferKind::Channel
                } else {
                    BufferKind::Query
                };
                self.ensure_buffer(id, &buffer, kind);
                self.emit(Event::History {
                    network: id.to_owned(),
                    buffer,
                    messages,
                });
            }
            // Other batches (netsplit, netjoin, …) hold ordinary live traffic.
            _ => {
                for msg in &batch.messages {
                    self.message(id, msg);
                }
            }
        }
    }

    fn message(&mut self, id: &str, msg: &Message) {
        match msg.command.as_str() {
            "NICK" => {
                if let Some(net) = self.networks.get_mut(id)
                    && msg
                        .source_nick()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&net.nick))
                    && let Some(new) = msg.param(0)
                {
                    net.nick = new.to_owned();
                }
            }
            "MARKREAD" => {
                let (Some(target), Some(ts)) = (msg.param(0), msg.param(1)) else {
                    return;
                };
                let time = ts
                    .strip_prefix("timestamp=")
                    .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.with_timezone(&Utc));
                self.emit(Event::ReadMarker {
                    network: id.to_owned(),
                    buffer: target.to_owned(),
                    time,
                });
            }
            "FAIL" => self.error(Some(id), msg.to_string()),
            _ => self.live(id, msg),
        }
    }

    /// Routes a chat message to its buffer and emits it.
    fn live(&mut self, id: &str, msg: &Message) {
        let Some(net) = self.networks.get(id) else {
            return;
        };
        let Some((buffer, kind, message)) = route(msg, &net.ctx()) else {
            return;
        };
        self.ensure_buffer(id, &buffer, kind);
        self.emit(Event::Message {
            network: id.to_owned(),
            buffer,
            message,
        });
    }

    fn ensure_buffer(&mut self, id: &str, name: &str, kind: BufferKind) {
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        if net.buffers.insert(buffer_key(name), kind).is_some() {
            return;
        }
        if kind != BufferKind::Server && net.caps.contains("draft/read-marker") {
            // Ask for the read marker; the reply is a MARKREAD.
            let _ = net
                .conn
                .send(ToConn::Send(Message::new("MARKREAD", [name])));
        }
        self.emit(Event::BufferAdded {
            network: id.to_owned(),
            name: name.to_owned(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    type Spawned = Arc<Mutex<Vec<(ConnId, mpsc::UnboundedReceiver<ToConn>)>>>;

    /// A coordinator whose connections are channels the test can inspect.
    struct Harness {
        c: Coordinator,
        events: mpsc::UnboundedReceiver<Event>,
        conns: Spawned,
    }

    impl Harness {
        fn new() -> Self {
            let conns = Arc::new(Mutex::new(Vec::new()));
            let spawned = conns.clone();
            let spawner: Spawner = Box::new(move |id, _| {
                let (tx, rx) = mpsc::unbounded_channel();
                spawned.lock().unwrap().push((id, rx));
                tx
            });
            let (from_tx, _from_rx) = mpsc::unbounded_channel();
            let (ev_tx, events) = mpsc::unbounded_channel();
            Harness {
                c: Coordinator::new(spawner, from_tx, ev_tx),
                events,
                conns,
            }
        }

        fn line(&mut self, id: Option<&str>, line: &str) {
            let msg = Message::parse(line).unwrap();
            self.c.on_connection(
                id.map(str::to_owned),
                FromConn::Incoming(Incoming::Message(msg)),
            );
        }

        fn events(&mut self) -> Vec<Event> {
            std::iter::from_fn(|| self.events.try_recv().ok()).collect()
        }

        /// Lines sent on connection `id`.
        fn sent(&self, id: Option<&str>) -> Vec<String> {
            let mut conns = self.conns.lock().unwrap();
            let (_, rx) = conns.iter_mut().find(|(c, _)| c.as_deref() == id).unwrap();
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|m| match m {
                    ToConn::Send(m) => m.to_string(),
                    ToConn::Stop => "<stop>".into(),
                })
                .collect()
        }

        /// Adds network 1 and brings it up with the given caps.
        fn connected_network(&mut self, caps: &[&str]) {
            self.line(None, "BOUNCER NETWORK 1 name=Libera;state=connected");
            let reg = Registered {
                nick: "me".into(),
                enabled_caps: caps.iter().map(|c| c.to_string()).collect(),
                isupport: [
                    ("CHANTYPES".into(), "#".into()),
                    ("CHATHISTORY".into(), "100".into()),
                ]
                .into(),
                ..Default::default()
            };
            self.c
                .on_connection(Some("1".into()), FromConn::Registered(Box::new(reg)));
            self.c.on_connection(
                Some("1".into()),
                FromConn::State(ConnectionState::Connected),
            );
            self.events();
            self.sent(Some("1"));
        }
    }

    #[test]
    fn networks_are_added_merged_and_removed() {
        let mut h = Harness::new();
        h.line(None, "BOUNCER NETWORK 1 name=Libera;state=connecting");
        h.line(None, "BOUNCER NETWORK 1 state=connected;error=");
        let ev = h.events();
        let Event::NetworkUpdated(info) = &ev[1] else {
            panic!("{ev:?}")
        };
        assert_eq!(info.name(), "Libera");
        assert_eq!(info.upstream_state(), Some("connected"));
        assert_eq!(h.conns.lock().unwrap().len(), 2, "control + one network");

        h.line(None, "BOUNCER NETWORK 1 *");
        assert_eq!(h.events(), [Event::NetworkRemoved("1".into())]);
        assert_eq!(h.sent(Some("1")), ["<stop>"]);
    }

    #[test]
    fn registration_requests_targets_and_markers() {
        let mut h = Harness::new();
        h.line(None, "BOUNCER NETWORK 1 name=Libera");
        let reg = Registered {
            nick: "me".into(),
            enabled_caps: ["draft/chathistory".into(), "draft/read-marker".into()].into(),
            ..Default::default()
        };
        h.c.on_connection(Some("1".into()), FromConn::Registered(Box::new(reg)));
        let sent = h.sent(Some("1"));
        assert!(
            sent[0].starts_with("CHATHISTORY TARGETS timestamp="),
            "{sent:?}"
        );
        assert_eq!(sent.len(), 1, "no MARKREAD for the server buffer: {sent:?}");
        assert!(h.events().contains(&Event::BufferAdded {
            network: "1".into(),
            name: SERVER_BUFFER.into(),
            kind: BufferKind::Server
        }));

        let mut batch = Batch {
            kind: "draft/chathistory-targets".into(),
            params: vec![],
            tags: vec![],
            messages: vec![
                Message::parse("CHATHISTORY TARGETS #rust timestamp=2026-10-01T00:00:00.000Z")
                    .unwrap(),
                Message::parse("CHATHISTORY TARGETS alice timestamp=2026-10-02T00:00:00.000Z")
                    .unwrap(),
            ],
            children: vec![],
        };
        h.c.on_connection(
            Some("1".into()),
            FromConn::Incoming(Incoming::Batch(batch.clone())),
        );
        let ev = h.events();
        assert!(ev.contains(&Event::BufferAdded {
            network: "1".into(),
            name: "#rust".into(),
            kind: BufferKind::Channel
        }));
        assert!(ev.contains(&Event::BufferAdded {
            network: "1".into(),
            name: "alice".into(),
            kind: BufferKind::Query
        }));
        assert_eq!(h.sent(Some("1")), ["MARKREAD #rust", "MARKREAD alice"]);

        // Known buffers aren't announced twice.
        batch.messages.truncate(1);
        h.c.on_connection(Some("1".into()), FromConn::Incoming(Incoming::Batch(batch)));
        assert!(h.events().is_empty());
    }

    #[test]
    fn live_messages_and_read_markers() {
        let mut h = Harness::new();
        h.connected_network(&["echo-message", "draft/read-marker"]);
        h.line(
            Some("1"),
            "@time=2026-10-24T10:00:00.000Z :alice!a@h PRIVMSG me :hi",
        );
        h.line(
            Some("1"),
            "MARKREAD alice timestamp=2026-10-24T10:00:00.000Z",
        );
        h.line(Some("1"), "MARKREAD #empty *");
        let ev = h.events();
        assert!(
            matches!(&ev[0], Event::BufferAdded { name, kind: BufferKind::Query, .. } if name == "alice")
        );
        let Event::Message {
            buffer, message, ..
        } = &ev[1]
        else {
            panic!("{ev:?}")
        };
        assert_eq!((buffer.as_str(), message.text.as_str()), ("alice", "hi"));
        let Event::ReadMarker { time: Some(t), .. } = &ev[2] else {
            panic!("{ev:?}")
        };
        assert_eq!(irc_timestamp(*t), "2026-10-24T10:00:00.000Z");
        assert!(matches!(&ev[3], Event::ReadMarker { time: None, .. }));
    }

    #[test]
    fn commands_become_irc_lines() {
        let mut h = Harness::new();
        h.connected_network(&["echo-message", "draft/chathistory", "draft/read-marker"]);
        let t = DateTime::parse_from_rfc3339("2026-10-24T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for cmd in [
            Command::SendMessage {
                network: "1".into(),
                target: "#rust".into(),
                text: "hello there".into(),
            },
            Command::Join {
                network: "1".into(),
                channel: "#new".into(),
            },
            Command::Part {
                network: "1".into(),
                channel: "#old".into(),
                reason: Some("bye now".into()),
            },
            Command::LoadHistory {
                network: "1".into(),
                target: "#rust".into(),
                before: None,
                limit: 500,
            },
            Command::LoadHistory {
                network: "1".into(),
                target: "#rust".into(),
                before: Some(t),
                limit: 50,
            },
            Command::MarkRead {
                network: "1".into(),
                target: "#rust".into(),
                time: t,
            },
        ] {
            h.c.command(cmd);
        }
        assert_eq!(
            h.sent(Some("1")),
            [
                "PRIVMSG #rust :hello there",
                "JOIN #new",
                "PART #old :bye now",
                // Clamped to ISUPPORT CHATHISTORY=100.
                "CHATHISTORY LATEST #rust * 100",
                "CHATHISTORY BEFORE #rust timestamp=2026-10-24T10:00:00.000Z 50",
                "MARKREAD #rust timestamp=2026-10-24T10:00:00.000Z",
            ]
        );
        // With echo-message, our own message arrives from the server instead.
        assert!(h.events().is_empty());
    }

    #[test]
    fn without_echo_message_own_messages_are_shown_immediately() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.c.command(Command::SendMessage {
            network: "1".into(),
            target: "#rust".into(),
            text: "hi".into(),
        });
        let ev = h.events();
        let Event::Message { message, .. } = &ev[1] else {
            panic!("{ev:?}")
        };
        assert!(message.own);
        assert_eq!(message.sender, "me");
    }

    #[test]
    fn rejects_bad_commands_without_sending() {
        let mut h = Harness::new();
        h.line(None, "BOUNCER NETWORK 1 name=Libera");
        h.events();
        // Not connected yet.
        h.c.command(Command::Join {
            network: "1".into(),
            channel: "#c".into(),
        });
        // Multi-line text.
        h.c.command(Command::SendMessage {
            network: "1".into(),
            target: "#c".into(),
            text: "a\r\nQUIT".into(),
        });
        h.c.command(Command::Join {
            network: "9".into(),
            channel: "#c".into(),
        });
        let ev = h.events();
        assert_eq!(ev.len(), 3);
        assert!(
            ev.iter().all(|e| matches!(e, Event::Error { .. })),
            "{ev:?}"
        );
        assert!(h.sent(Some("1")).is_empty());
    }

    #[test]
    fn history_batches_become_history_events() {
        let mut h = Harness::new();
        h.connected_network(&["draft/chathistory"]);
        let batch = Batch {
            kind: "chathistory".into(),
            params: vec!["#rust".into()],
            tags: vec![],
            messages: vec![
                Message::parse("@time=2026-10-24T09:00:00.000Z :alice!a@h PRIVMSG #rust :one")
                    .unwrap(),
                Message::parse("@time=2026-10-24T09:01:00.000Z :me!m@h PRIVMSG #rust :two")
                    .unwrap(),
            ],
            children: vec![],
        };
        h.c.on_connection(Some("1".into()), FromConn::Incoming(Incoming::Batch(batch)));
        let ev = h.events();
        let Event::History {
            buffer, messages, ..
        } = &ev[1]
        else {
            panic!("{ev:?}")
        };
        assert_eq!(buffer, "#rust");
        assert_eq!(
            messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert!(messages[1].own);
    }
}
