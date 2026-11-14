//! Owns the session state and routes between the front end and connections.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use super::connection::{self, ConnId, FromConn, FromConnTx, ToConn};
use super::members::{Members, WHO_TOKEN};
use super::{Command, ConnectionState, Event, SessionConfig};
use crate::batch::Batch;
use crate::bouncer::{NetworkUpdate, parse_network_message};
use crate::client::Incoming;
use crate::model::{
    BufferKind, ChannelListing, ChatMessage, MessageKind, NetworkId, NetworkInfo, RouteContext,
    SERVER_BUFFER, WhoisInfo, buffer_key, irc_timestamp, quit_or_nick, route, route_reply,
};
use crate::proto::Message;
use crate::proto::message::escape_tag_value;
use crate::session::Registered;

/// `RPL_LIST` entries per [`Event::ChannelList`]: few enough events for
/// a 50,000-channel network, each small enough to apply quickly.
const LIST_CHUNK: usize = 500;

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
    /// Channels the user asked to join ([`buffer_key`]s). Only their JOIN
    /// echoes are shown: soju replays a JOIN for every channel on each new
    /// connection, and those aren't news.
    requested_joins: HashSet<String>,
    members: Members,
    /// `RPL_LIST` entries not yet sent to the front end.
    channel_list: Vec<ChannelListing>,
    /// ISUPPORT `WHOX`: WHO can ask for just the fields we need.
    whox: bool,
    /// Channels (keys) we sent a WHO for on this connection, to learn who's
    /// away, and those whose replies are still coming (not shown).
    who_requested: HashSet<String>,
    who_pending: HashSet<String>,
    /// WHOIS replies being collected, by nick key.
    whois: HashMap<String, WhoisInfo>,
}

impl Network {
    /// Applies one ISUPPORT token (`None`: removed), at registration or
    /// later: soju re-sends them once it reaches the upstream network.
    fn set_isupport(&mut self, key: &str, value: Option<&str>) {
        match key {
            "CHANTYPES" => {
                self.chantypes = value.filter(|c| !c.is_empty()).unwrap_or("#&").to_owned();
            }
            "CHATHISTORY" => {
                self.history_max = value.and_then(|v| v.parse().ok()).filter(|&n| n > 0);
            }
            "PREFIX" | "CHANMODES" => self.members.set_isupport(key, value),
            "WHOX" => self.whox = value.is_some(),
            _ => {}
        }
    }

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
            Command::Join {
                network,
                channel,
                key,
            } => {
                let key = key.filter(|k| !k.is_empty());
                let bad = |s: &str| s.is_empty() || s.contains([' ', '\r', '\n', '\0']);
                if bad(&channel) || key.as_deref().is_some_and(bad) {
                    return self.error(
                        Some(&network),
                        "channel names and keys can't contain spaces",
                    );
                }
                if let Some(net) = self.networks.get_mut(&network) {
                    net.requested_joins
                        .extend(channel.split(',').map(buffer_key));
                }
                let mut params = vec![channel];
                params.extend(key);
                self.send_to(&network, Message::new("JOIN", params));
            }
            Command::ListChannels { network } => {
                if let Some(net) = self.networks.get_mut(&network) {
                    net.channel_list.clear();
                }
                self.send_to(&network, Message::new("LIST", Vec::<String>::new()));
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
            Command::AddNetwork { attributes } => {
                let attrs = attributes
                    .iter()
                    .map(|(k, v)| format!("{k}={}", escape_tag_value(v)))
                    .collect::<Vec<_>>()
                    .join(";");
                let msg = Message::new("BOUNCER", ["ADDNETWORK".to_owned(), attrs]);
                let _ = self.control.send(ToConn::Send(msg));
            }
            Command::ChangeNetwork {
                network,
                attributes,
            } => {
                let attrs = attributes
                    .iter()
                    .map(|(k, v)| format!("{k}={}", escape_tag_value(v)))
                    .collect::<Vec<_>>()
                    .join(";");
                let msg = Message::new("BOUNCER", ["CHANGENETWORK".to_owned(), network, attrs]);
                let _ = self.control.send(ToConn::Send(msg));
            }
            Command::RemoveNetwork { network } => {
                let msg = Message::new("BOUNCER", ["DELNETWORK".to_owned(), network]);
                let _ = self.control.send(ToConn::Send(msg));
            }
            Command::SendRaw { network, line } => match Message::parse(line.trim()) {
                Ok(msg) => {
                    self.send_to(&network, msg);
                }
                Err(_) => self.error(Some(&network), format!("not a valid IRC line: {line}")),
            },
            Command::CloseBuffer { network, name } => {
                let is_channel = self
                    .networks
                    .get(&network)
                    .is_some_and(|net| net.ctx().is_channel(&name));
                if is_channel {
                    // Removed when the server confirms the PART.
                    self.send_to(&network, Message::new("PART", [name]));
                } else {
                    self.remove_buffer(&network, &name);
                }
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
            (None, FromConn::Incoming(Incoming::Message(msg))) if msg.command == "FAIL" => {
                // e.g. FAIL BOUNCER INVALID_ATTRIBUTE … :description
                let text = msg
                    .params
                    .last()
                    .cloned()
                    .unwrap_or_else(|| msg.to_string());
                self.error(None, text);
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
                                requested_joins: HashSet::new(),
                                members: Members::new(&BTreeMap::new()),
                                channel_list: Vec::new(),
                                whox: false,
                                who_requested: HashSet::new(),
                                who_pending: HashSet::new(),
                                whois: HashMap::new(),
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
        net.members = Members::new(&reg.isupport);
        net.who_requested.clear();
        net.who_pending.clear();
        net.whois.clear();
        net.whox = false;
        for (key, value) in &reg.isupport {
            net.set_isupport(key, Some(value));
        }
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
        if let Some(net) = self.networks.get(id) {
            self.emit(Event::OwnNick {
                network: id.to_owned(),
                nick: net.nick.clone(),
            });
        }
    }

    fn batch(&mut self, id: &str, batch: Batch) {
        match batch.kind.as_str() {
            "draft/chathistory-targets" => {
                for msg in &batch.messages {
                    // CHATHISTORY TARGETS <target> <latest timestamp>
                    // Only private conversations: channel buffers come from
                    // JOINs, so channels we've left don't come back.
                    if msg.command == "CHATHISTORY"
                        && msg.param(0) == Some("TARGETS")
                        && let Some(target) = msg.param(1)
                        && self
                            .networks
                            .get(id)
                            .is_some_and(|net| !net.ctx().is_channel(target))
                    {
                        self.ensure_buffer(id, target, BufferKind::Query);
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
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        // Where a QUIT or NICK shows: the channels they shared with us (known
        // only before the member lists are updated) and our conversation.
        let fan_out = quit_or_nick(msg, &net.ctx()).map(|chat| {
            let mut buffers = net.members.channels_of(&chat.sender);
            if net.buffers.get(&buffer_key(&chat.sender)) == Some(&BufferKind::Query) {
                buffers.push(chat.sender.clone());
            }
            (chat, buffers)
        });
        let changed = net.members.apply(msg, &net.nick);
        let left = left_channel(msg, &net.nick);
        self.dispatch(id, msg);
        if let Some((chat, buffers)) = fan_out {
            for buffer in buffers {
                self.emit(Event::Message {
                    network: id.to_owned(),
                    buffer,
                    message: chat.clone(),
                });
            }
        }
        for channel in changed {
            let members = match self.networks.get(id) {
                Some(net) => net.members.list(&channel),
                None => continue,
            };
            self.emit(Event::Members {
                network: id.to_owned(),
                channel,
                members,
            });
        }
        if let Some(channel) = left {
            self.remove_buffer(id, &channel);
        }
    }

    fn dispatch(&mut self, id: &str, msg: &Message) {
        match msg.command.as_str() {
            "NICK" => {
                if let Some(net) = self.networks.get_mut(id)
                    && msg
                        .source_nick()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&net.nick))
                    && let Some(new) = msg.param(0)
                {
                    net.nick = new.to_owned();
                    self.emit(Event::OwnNick {
                        network: id.to_owned(),
                        nick: new.to_owned(),
                    });
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
            // RPL_LIST <me> <channel> <users> :<topic>
            "322" => {
                let (Some(net), Some(name)) = (self.networks.get_mut(id), msg.param(1)) else {
                    return;
                };
                net.channel_list.push(ChannelListing {
                    name: name.to_owned(),
                    users: msg.param(2).and_then(|n| n.parse().ok()).unwrap_or(0),
                    topic: msg.param(3).unwrap_or("").to_owned(),
                });
                if net.channel_list.len() >= LIST_CHUNK {
                    self.flush_channel_list(id, false);
                }
            }
            // RPL_LISTEND
            "323" => self.flush_channel_list(id, true),
            // RPL_LISTSTART: nothing to do.
            "321" => {}
            // Handled by the member lists, or noise after joining: names,
            // topic setter, creation time, channel modes.
            "353" | "333" | "329" | "324" => {}
            // RPL_ENDOFNAMES: the list is complete; learn who's away.
            "366" => {
                if let Some(chan) = msg.param(1) {
                    self.request_who(id, chan);
                }
            }
            // WHO replies we asked for aren't shown.
            "352" | "315" if self.who_pending(id, msg.param(1)) => {
                if msg.command == "315"
                    && let (Some(net), Some(chan)) = (self.networks.get_mut(id), msg.param(1))
                {
                    net.who_pending.remove(&buffer_key(chan));
                }
            }
            "354" if msg.param(1) == Some(WHO_TOKEN) => {}
            // RPL_UNAWAY, RPL_NOWAWAY
            "305" | "306" => {
                self.emit(Event::Away {
                    network: id.to_owned(),
                    away: msg.command == "306",
                });
                self.reply(id, msg);
            }
            // RPL_WHOISUSER starts a WHOIS; RPL_ENDOFWHOIS ends it.
            "311" => {
                if let (Some(net), Some(nick)) = (self.networks.get_mut(id), msg.param(1)) {
                    net.whois.insert(
                        buffer_key(nick),
                        WhoisInfo {
                            nick: nick.to_owned(),
                            user: msg.param(2).map(str::to_owned),
                            host: msg.param(3).map(str::to_owned),
                            realname: msg.params.last().cloned(),
                            ..Default::default()
                        },
                    );
                }
            }
            "318" => {
                let info = msg.param(1).and_then(|nick| {
                    self.networks
                        .get_mut(id)
                        .and_then(|net| net.whois.remove(&buffer_key(nick)))
                });
                if let Some(info) = info {
                    self.emit(Event::Whois {
                        network: id.to_owned(),
                        info,
                    });
                }
            }
            // RPL_TOPIC <me> <channel> :<topic>, RPL_NOTOPIC <me> <channel> :…
            "332" | "331" => {
                if let Some(channel) = msg.param(1) {
                    let topic = if msg.command == "332" {
                        msg.param(2)
                    } else {
                        None
                    };
                    self.topic(id, channel, topic);
                }
            }
            "TOPIC" => {
                if let Some(channel) = msg.param(0) {
                    self.topic(id, channel, msg.param(1));
                }
                self.live(id, msg);
            }
            // RPL_ISUPPORT <me> <token>… :are supported by this server
            "005" => {
                if let Some(net) = self.networks.get_mut(id) {
                    let tokens = msg
                        .params
                        .iter()
                        .skip(1)
                        .take(msg.params.len().saturating_sub(2));
                    for token in tokens {
                        match token.strip_prefix('-') {
                            Some(key) => net.set_isupport(key, None),
                            None => {
                                let (key, value) = token.split_once('=').unwrap_or((token, ""));
                                net.set_isupport(key, Some(value));
                            }
                        }
                    }
                }
            }
            // Any other numeric: part of a WHOIS, or a reply to show.
            code if code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()) => {
                if !self.whois_line(id, msg) {
                    self.reply(id, msg);
                }
            }
            _ => self.live(id, msg),
        }
    }

    /// Asks who's away in a channel we just joined: once per connection,
    /// only with `away-notify` (so it stays current) and not for big
    /// channels, where the reply would be thousands of lines.
    fn request_who(&mut self, id: &str, chan: &str) {
        const WHO_LIMIT: usize = 500;
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        let key = buffer_key(chan);
        if !net.caps.contains("away-notify")
            || net.members.count(chan) > WHO_LIMIT
            || !net.who_requested.insert(key.clone())
        {
            return;
        }
        net.who_pending.insert(key);
        let msg = if net.whox {
            Message::new("WHO", [chan.to_owned(), format!("%tcnf,{WHO_TOKEN}")])
        } else {
            Message::new("WHO", [chan])
        };
        let _ = net.conn.send(ToConn::Send(msg));
    }

    fn who_pending(&self, id: &str, chan: Option<&str>) -> bool {
        match (self.networks.get(id), chan) {
            (Some(net), Some(chan)) => net.who_pending.contains(&buffer_key(chan)),
            _ => false,
        }
    }

    /// Adds a WHOIS numeric to the reply being collected. False if there
    /// is none for that nick (e.g. `RPL_AWAY` after messaging someone).
    fn whois_line(&mut self, id: &str, msg: &Message) -> bool {
        let Some(net) = self.networks.get_mut(id) else {
            return false;
        };
        let Some(info) = msg.param(1).and_then(|n| net.whois.get_mut(&buffer_key(n))) else {
            return false;
        };
        let last = msg.params.last().cloned().unwrap_or_default();
        match msg.command.as_str() {
            // <me> <nick> <server> :<info>
            "312" => {
                info.server = msg.param(2).map(str::to_owned);
                info.server_info = Some(last);
            }
            "313" => info.operator = true,
            // <me> <nick> <idle> <signon> :seconds idle, signon time
            "317" => {
                info.idle_secs = msg.param(2).and_then(|s| s.parse().ok());
                info.signon = msg
                    .param(3)
                    .and_then(|s| s.parse().ok())
                    .and_then(|t| DateTime::from_timestamp(t, 0));
            }
            "319" => info
                .channels
                .extend(last.split_whitespace().map(str::to_owned)),
            "301" => info.away = Some(last),
            // <me> <nick> <account> :is logged in as
            "330" => info.account = msg.param(2).map(str::to_owned),
            "671" => info.secure = true,
            _ => {
                let middle = msg
                    .params
                    .get(2..msg.params.len().saturating_sub(1))
                    .unwrap_or(&[]);
                info.other.push(if middle.is_empty() {
                    last
                } else {
                    format!("{} {last}", middle.join(" "))
                });
            }
        }
        true
    }

    /// Shows a numeric reply: errors about a channel or conversation there,
    /// `RPL_AWAY` in the conversation with that person, the rest in the
    /// server buffer.
    fn reply(&mut self, id: &str, msg: &Message) {
        let Some(net) = self.networks.get(id) else {
            return;
        };
        let has_buffer = |name: &str| net.buffers.get(&buffer_key(name)).copied();
        let Some((mut buffer, mut kind, mut chat)) = route_reply(msg, has_buffer) else {
            return;
        };
        // RPL_AWAY <me> <nick> :<message>, after we messaged them.
        if msg.command == "301"
            && let Some(nick) = msg.param(1)
            && net.buffers.get(&buffer_key(nick)) == Some(&BufferKind::Query)
        {
            buffer = nick.to_owned();
            kind = BufferKind::Query;
            chat.text = format!(
                "{nick} is away: {}",
                msg.params.last().map_or("", |s| s.as_str())
            );
        }
        self.ensure_buffer(id, &buffer, kind);
        self.emit(Event::Message {
            network: id.to_owned(),
            buffer,
            message: chat,
        });
    }

    fn flush_channel_list(&mut self, id: &str, done: bool) {
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        let channels = std::mem::take(&mut net.channel_list);
        self.emit(Event::ChannelList {
            network: id.to_owned(),
            channels,
            done,
        });
    }

    /// Routes a chat message to its buffer and emits it.
    fn live(&mut self, id: &str, msg: &Message) {
        let Some(net) = self.networks.get(id) else {
            return;
        };
        let Some((buffer, kind, message)) = route(msg, &net.ctx()) else {
            return;
        };
        let replayed_join = message.own
            && message.kind == MessageKind::Join
            && !self
                .networks
                .get_mut(id)
                .is_some_and(|net| net.requested_joins.remove(&buffer_key(&buffer)));
        self.ensure_buffer(id, &buffer, kind);
        if replayed_join {
            return;
        }
        self.emit(Event::Message {
            network: id.to_owned(),
            buffer,
            message,
        });
    }

    fn topic(&mut self, id: &str, channel: &str, topic: Option<&str>) {
        self.emit(Event::Topic {
            network: id.to_owned(),
            channel: channel.to_owned(),
            topic: topic.filter(|t| !t.is_empty()).map(str::to_owned),
        });
    }

    fn remove_buffer(&mut self, id: &str, name: &str) {
        let Some(net) = self.networks.get_mut(id) else {
            return;
        };
        if net.buffers.remove(&buffer_key(name)).is_some() {
            self.emit(Event::BufferRemoved {
                network: id.to_owned(),
                name: name.to_owned(),
            });
        }
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

/// The channel `msg` says we left (our PART, or a KICK of us).
fn left_channel(msg: &Message, own_nick: &str) -> Option<String> {
    let own = |nick: &str| nick.eq_ignore_ascii_case(own_nick);
    match msg.command.as_str() {
        "PART" if msg.source_nick().is_some_and(own) => msg.param(0).map(str::to_owned),
        "KICK" if msg.param(1).is_some_and(own) => msg.param(0).map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::model::Member;

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
        // Channels come from JOINs, so ones we've left don't come back.
        assert!(
            !ev.iter()
                .any(|e| matches!(e, Event::BufferAdded { name, .. } if name == "#rust"))
        );
        assert!(ev.contains(&Event::BufferAdded {
            network: "1".into(),
            name: "alice".into(),
            kind: BufferKind::Query
        }));
        assert_eq!(h.sent(Some("1")), ["MARKREAD alice"]);

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
    fn members_and_leaving() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.line(Some("1"), ":me!u@h JOIN #rust");
        h.line(Some("1"), ":srv 353 me = #rust :@op me");
        h.line(Some("1"), ":srv 366 me #rust :End");
        let ev = h.events();
        assert!(
            ev.contains(&Event::Members {
                network: "1".into(),
                channel: "#rust".into(),
                members: vec![
                    Member {
                        nick: "op".into(),
                        prefix: "@".into(),
                        away: false,
                    },
                    Member {
                        nick: "me".into(),
                        prefix: String::new(),
                        away: false,
                    },
                ],
            }),
            "{ev:?}"
        );

        // Closing a channel parts it; the buffer goes when the server agrees.
        h.c.command(Command::CloseBuffer {
            network: "1".into(),
            name: "#rust".into(),
        });
        assert_eq!(h.sent(Some("1")), ["PART #rust"]);
        assert!(h.events().is_empty());
        h.line(Some("1"), ":me!u@h PART #rust");
        assert!(h.events().contains(&Event::BufferRemoved {
            network: "1".into(),
            name: "#rust".into()
        }));

        // Being kicked removes it too.
        h.line(Some("1"), ":me!u@h JOIN #go");
        h.line(Some("1"), ":op!o@h KICK #go me :bye");
        assert!(h.events().contains(&Event::BufferRemoved {
            network: "1".into(),
            name: "#go".into()
        }));

        // Closing a private conversation just forgets it.
        h.line(Some("1"), ":erin!e@h PRIVMSG me :hi");
        h.events();
        h.c.command(Command::CloseBuffer {
            network: "1".into(),
            name: "erin".into(),
        });
        assert!(h.sent(Some("1")).is_empty());
        assert_eq!(
            h.events(),
            [Event::BufferRemoved {
                network: "1".into(),
                name: "erin".into()
            }]
        );
    }

    #[test]
    fn topics() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.line(Some("1"), ":srv 332 me #rust :Rust talk");
        h.line(Some("1"), ":srv 331 me #empty :No topic is set");
        let topic = |channel: &str, topic: Option<&str>| Event::Topic {
            network: "1".into(),
            channel: channel.into(),
            topic: topic.map(Into::into),
        };
        assert_eq!(
            h.events(),
            [topic("#rust", Some("Rust talk")), topic("#empty", None)]
        );
        // A change is both a topic and a line in the channel.
        h.line(Some("1"), ":op!o@h TOPIC #rust :New topic");
        let ev = h.events();
        assert!(ev.contains(&topic("#rust", Some("New topic"))), "{ev:?}");
        assert!(ev.iter().any(
            |e| matches!(e, Event::Message { message, .. } if message.kind == MessageKind::Topic)
        ));
        // Clearing it.
        h.line(Some("1"), ":op!o@h TOPIC #rust :");
        assert!(h.events().contains(&topic("#rust", None)));
    }

    #[test]
    fn replayed_own_joins_are_not_messages() {
        let mut h = Harness::new();
        h.connected_network(&["echo-message"]);

        // soju replays our channels on connect: buffer yes, message no.
        h.line(Some("1"), ":me!u@h JOIN #rust");
        assert_eq!(
            h.events(),
            [Event::BufferAdded {
                network: "1".into(),
                name: "#rust".into(),
                kind: BufferKind::Channel
            }]
        );

        // A join the user asked for is shown, once.
        h.c.command(Command::Join {
            network: "1".into(),
            channel: "#New,#other".into(),
            key: None,
        });
        h.line(Some("1"), ":me!u@h JOIN #new");
        let ev = h.events();
        assert!(
            ev.iter()
                .any(|e| matches!(e, Event::Message { buffer, message, .. }
                if buffer == "#new" && message.kind == MessageKind::Join)),
            "{ev:?}"
        );
        h.line(Some("1"), ":me!u@h JOIN #new");
        assert!(h.events().is_empty(), "a second replay is quiet");

        // Other people's joins are always shown.
        h.line(Some("1"), ":erin!u@h JOIN #rust");
        assert!(
            h.events()
                .iter()
                .any(|e| matches!(e, Event::Message { .. }))
        );
    }

    #[test]
    fn network_management_and_raw_lines() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.c.command(Command::AddNetwork {
            attributes: [
                ("name".to_owned(), "Libera Chat".to_owned()),
                ("host".to_owned(), "irc.libera.chat".to_owned()),
                ("tls".to_owned(), "1".to_owned()),
            ]
            .into(),
        });
        h.c.command(Command::ChangeNetwork {
            network: "1".into(),
            attributes: [
                ("port".to_owned(), "6667".to_owned()),
                ("nickname".to_owned(), String::new()),
            ]
            .into(),
        });
        h.c.command(Command::RemoveNetwork {
            network: "1".into(),
        });
        assert_eq!(
            h.sent(None),
            [
                "BOUNCER ADDNETWORK host=irc.libera.chat;name=Libera\\sChat;tls=1",
                // An empty value removes the attribute.
                "BOUNCER CHANGENETWORK 1 nickname=;port=6667",
                "BOUNCER DELNETWORK 1",
            ]
        );

        // ISUPPORT arriving after registration still counts.
        h.line(
            Some("1"),
            ":srv 005 me PREFIX=(Yo)!@ CHANTYPES=#& :are supported",
        );
        h.line(Some("1"), ":me!u@h JOIN #x");
        h.line(Some("1"), ":srv 353 me = #x :!boss @op me");
        h.line(Some("1"), ":srv 366 me #x :End");
        let members = h.events().into_iter().find_map(|e| match e {
            Event::Members { members, .. } => Some(members),
            _ => None,
        });
        let nicks: Vec<String> = members
            .unwrap()
            .into_iter()
            .map(|m| m.prefix + &m.nick)
            .collect();
        assert_eq!(nicks, ["!boss", "@op", "me"]);

        h.c.command(Command::SendRaw {
            network: "1".into(),
            line: "WHOIS alice".into(),
        });
        assert_eq!(h.sent(Some("1")), ["WHOIS alice"]);

        // A rejection from soju reaches the front end.
        h.line(
            None,
            "FAIL BOUNCER INVALID_ATTRIBUTE ADDNETWORK host :Invalid host",
        );
        assert_eq!(
            h.events(),
            [Event::Error {
                network: None,
                message: "Invalid host".into()
            }]
        );
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
                key: None,
            },
            Command::Join {
                network: "1".into(),
                channel: "#secret".into(),
                key: Some("sesame".into()),
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
                "JOIN #secret sesame",
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

    /// Messages emitted, as (buffer, kind, sender, text).
    fn messages(ev: &[Event]) -> Vec<(String, MessageKind, String, String)> {
        ev.iter()
            .filter_map(|e| match e {
                Event::Message {
                    buffer, message, ..
                } => Some((
                    buffer.clone(),
                    message.kind,
                    message.sender.clone(),
                    message.text.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn joined_with(h: &mut Harness, chan: &str, names: &str) {
        h.line(Some("1"), &format!(":me!u@h JOIN {chan}"));
        h.line(Some("1"), &format!(":srv 353 me = {chan} :{names}"));
        h.line(Some("1"), &format!(":srv 366 me {chan} :End"));
    }

    #[test]
    fn quits_and_nick_changes_show_where_they_matter() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        joined_with(&mut h, "#a", "alice bob me");
        joined_with(&mut h, "#b", "alice me");
        h.line(Some("1"), ":alice!u@h PRIVMSG me :hi");
        h.events();

        h.line(Some("1"), ":alice!u@h NICK alicia");
        assert_eq!(
            messages(&h.events()),
            [
                (
                    "#a".into(),
                    MessageKind::Nick,
                    "alice".into(),
                    "alicia".into()
                ),
                (
                    "#b".into(),
                    MessageKind::Nick,
                    "alice".into(),
                    "alicia".into()
                ),
                (
                    "alice".into(),
                    MessageKind::Nick,
                    "alice".into(),
                    "alicia".into()
                ),
            ]
        );
        h.line(Some("1"), ":bob!u@h QUIT :Ping timeout");
        assert_eq!(
            messages(&h.events()),
            [(
                "#a".into(),
                MessageKind::Quit,
                "bob".into(),
                "Ping timeout".into()
            )]
        );
        // Our own nick change is announced to the front end.
        h.line(Some("1"), ":me!u@h NICK me2");
        assert!(h.events().contains(&Event::OwnNick {
            network: "1".into(),
            nick: "me2".into()
        }));
    }

    #[test]
    fn replies_go_to_the_server_buffer_and_errors_to_their_channel() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        joined_with(&mut h, "#a", "me");
        h.events();
        h.line(Some("1"), ":srv 404 me #a :Cannot send to channel");
        h.line(Some("1"), ":srv 433 me * taken :Nickname is already in use");
        h.line(Some("1"), ":srv 306 me :You have been marked as being away");
        let ev = h.events();
        assert_eq!(
            messages(&ev),
            [
                (
                    "#a".into(),
                    MessageKind::Error,
                    "srv".into(),
                    "Cannot send to channel".into()
                ),
                (
                    "*".into(),
                    MessageKind::Error,
                    "srv".into(),
                    "taken: Nickname is already in use".into()
                ),
                (
                    "*".into(),
                    MessageKind::Reply,
                    "srv".into(),
                    "You have been marked as being away".into()
                ),
            ]
        );
        assert!(ev.contains(&Event::Away {
            network: "1".into(),
            away: true
        }));
        // NAMES and topic details after a join aren't shown.
        h.line(Some("1"), ":srv 333 me #a op 1700000000");
        assert!(messages(&h.events()).is_empty());
    }

    #[test]
    fn whois_is_collected_into_one_event() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        for line in [
            ":srv 311 me Bob bob host.example * :Bob Smith",
            ":srv 312 me Bob irc.example :Example server",
            ":srv 319 me Bob :@#ops #general",
            ":srv 317 me Bob 120 1700000000 :seconds idle, signon time",
            ":srv 301 me Bob :gone fishing",
            ":srv 330 me Bob bobby :is logged in as",
            ":srv 671 me Bob :is using a secure connection",
            ":srv 338 me Bob 192.0.2.1 :actually using host",
            ":srv 318 me Bob :End of /WHOIS list.",
        ] {
            h.line(Some("1"), line);
        }
        let ev = h.events();
        assert_eq!(ev.len(), 1, "{ev:?}");
        let Event::Whois { info, .. } = &ev[0] else {
            panic!("{ev:?}")
        };
        assert_eq!(info.nick, "Bob");
        assert_eq!(info.host.as_deref(), Some("host.example"));
        assert_eq!(info.realname.as_deref(), Some("Bob Smith"));
        assert_eq!(info.server.as_deref(), Some("irc.example"));
        assert_eq!(info.channels, ["@#ops", "#general"]);
        assert_eq!(info.idle_secs, Some(120));
        assert_eq!(info.signon.map(|t| t.timestamp()), Some(1_700_000_000));
        assert_eq!(info.away.as_deref(), Some("gone fishing"));
        assert_eq!(info.account.as_deref(), Some("bobby"));
        assert!(info.secure && !info.operator);
        assert_eq!(info.other, ["192.0.2.1 actually using host"]);

        // RPL_AWAY outside a WHOIS goes to the conversation.
        h.line(Some("1"), ":alice!u@h PRIVMSG me :hi");
        h.events();
        h.line(Some("1"), ":srv 301 me alice :lunch");
        assert_eq!(
            messages(&h.events()),
            [(
                "alice".into(),
                MessageKind::Reply,
                "srv".into(),
                "alice is away: lunch".into()
            )]
        );
    }

    #[test]
    fn small_channels_get_a_quiet_who() {
        let mut h = Harness::new();
        h.connected_network(&["away-notify"]);
        h.c.on_connection(
            Some("1".into()),
            FromConn::Incoming(Incoming::Message(
                Message::parse(":srv 005 me WHOX :are supported").unwrap(),
            )),
        );
        joined_with(&mut h, "#a", "alice me");
        assert_eq!(h.sent(Some("1")), ["WHO #a %tcnf,273"]);
        h.events();
        h.line(Some("1"), ":srv 354 me 273 #a alice G");
        h.line(Some("1"), ":srv 315 me #a :End of WHO list");
        let ev = h.events();
        assert!(messages(&ev).is_empty(), "WHO replies aren't shown: {ev:?}");
        assert!(
            ev.iter().any(|e| matches!(e,
            Event::Members { members, .. } if members.iter().any(|m| m.nick == "alice" && m.away)))
        );
        // Once per connection.
        joined_with(&mut h, "#a", "alice me");
        assert!(!h.sent(Some("1")).iter().any(|l| l.starts_with("WHO")));
    }

    #[test]
    fn channel_list_arrives_in_chunks() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.c.command(Command::ListChannels {
            network: "1".into(),
        });
        assert_eq!(h.sent(Some("1")), ["LIST"]);

        h.line(Some("1"), ":srv 321 me Channel :Users  Name");
        for i in 0..LIST_CHUNK + 1 {
            h.line(
                Some("1"),
                &format!(":srv 322 me #c{i} {i} :[+nt] topic {i}"),
            );
        }
        h.line(Some("1"), ":srv 323 me :End of /LIST");
        let ev = h.events();
        let chunks: Vec<_> = ev
            .iter()
            .map(|e| match e {
                Event::ChannelList { channels, done, .. } => (channels.len(), *done),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(chunks, [(LIST_CHUNK, false), (1, true)]);
        let Event::ChannelList { channels, .. } = &ev[0] else {
            unreachable!()
        };
        assert_eq!(
            channels[7],
            ChannelListing {
                name: "#c7".into(),
                users: 7,
                topic: "[+nt] topic 7".into(),
            }
        );
    }

    #[test]
    fn join_rejects_spaces_in_keys() {
        let mut h = Harness::new();
        h.connected_network(&[]);
        h.c.command(Command::Join {
            network: "1".into(),
            channel: "#c".into(),
            key: Some("two words".into()),
        });
        assert!(matches!(h.events()[..], [Event::Error { .. }]));
        assert!(h.sent(Some("1")).is_empty());
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
            key: None,
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
            key: None,
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
