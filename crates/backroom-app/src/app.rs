//! All app state, independent of any toolkit. The front end owns one
//! [`AppState`] on its UI thread, feeds it the session's events
//! ([`AppState::apply`]) and the user's actions, then redraws what
//! [`AppState::take_changes`] says changed.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use backroom_ffi::{
    BackroomClient, BackroomError, BanEntry, BufferKind, ChannelListing, ChatMessage,
    ConnectionState, MessageKind, SearchHit, SessionEvent, TypingState, WhoisInfo,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::buffer::{BufferId, BufferModel, NetworkState, Typing, is_chat, stable_id};
use crate::composer::{self, ComposerAction};
use crate::mentions::MentionMatcher;
use crate::panes::{PaneDirection, PaneLayout, SplitAxis, WorkspaceTab};
use crate::prefs::Preferences;
use crate::servers::{Login, NetworkFields, ServerList, ServerSettings};
use crate::storage::{self, Paths};

pub type ClientResult = Result<(), BackroomError>;

/// The session commands the app sends; [`BackroomClient`] in the app, a
/// fake in tests.
pub trait Client: Send + Sync {
    fn send_message(&self, network: &str, target: &str, text: &str) -> ClientResult;
    fn join(&self, network: &str, channel: &str, key: Option<&str>) -> ClientResult;
    fn part(&self, network: &str, channel: &str, reason: Option<&str>) -> ClientResult;
    fn search(&self, network: &str, target: &str, text: &str) -> ClientResult;
    fn set_typing(&self, network: &str, target: &str, state: TypingState) -> ClientResult;
    fn load_channel_settings(&self, network: &str, channel: &str) -> ClientResult;
    fn list_channels(&self, network: &str) -> ClientResult;
    fn add_network(&self, attributes: HashMap<String, String>) -> ClientResult;
    fn change_network(&self, network: &str, attributes: HashMap<String, String>) -> ClientResult;
    fn remove_network(&self, network: &str) -> ClientResult;
    fn send_raw(&self, network: &str, line: &str) -> ClientResult;
    fn close_buffer(&self, network: &str, name: &str) -> ClientResult;
    fn load_history(
        &self,
        network: &str,
        target: &str,
        before: Option<SystemTime>,
        limit: u32,
    ) -> ClientResult;
    fn mark_read(&self, network: &str, target: &str, time: SystemTime) -> ClientResult;
    fn set_text_logs(&self, folder: Option<String>);
}

impl Client for BackroomClient {
    fn send_message(&self, network: &str, target: &str, text: &str) -> ClientResult {
        self.send_message(network.into(), target.into(), text.into())
    }
    fn join(&self, network: &str, channel: &str, key: Option<&str>) -> ClientResult {
        self.join(network.into(), channel.into(), key.map(Into::into))
    }
    fn part(&self, network: &str, channel: &str, reason: Option<&str>) -> ClientResult {
        self.part(network.into(), channel.into(), reason.map(Into::into))
    }
    fn search(&self, network: &str, target: &str, text: &str) -> ClientResult {
        self.search(network.into(), target.into(), text.into())
    }
    fn set_typing(&self, network: &str, target: &str, state: TypingState) -> ClientResult {
        self.set_typing(network.into(), target.into(), state)
    }
    fn load_channel_settings(&self, network: &str, channel: &str) -> ClientResult {
        self.load_channel_settings(network.into(), channel.into())
    }
    fn list_channels(&self, network: &str) -> ClientResult {
        self.list_channels(network.into())
    }
    fn add_network(&self, attributes: HashMap<String, String>) -> ClientResult {
        self.add_network(attributes)
    }
    fn change_network(&self, network: &str, attributes: HashMap<String, String>) -> ClientResult {
        self.change_network(network.into(), attributes)
    }
    fn remove_network(&self, network: &str) -> ClientResult {
        self.remove_network(network.into())
    }
    fn send_raw(&self, network: &str, line: &str) -> ClientResult {
        self.send_raw(network.into(), line.into())
    }
    fn close_buffer(&self, network: &str, name: &str) -> ClientResult {
        self.close_buffer(network.into(), name.into())
    }
    fn load_history(
        &self,
        network: &str,
        target: &str,
        before: Option<SystemTime>,
        limit: u32,
    ) -> ClientResult {
        self.load_history(network.into(), target.into(), before, limit)
    }
    fn mark_read(&self, network: &str, target: &str, time: SystemTime) -> ClientResult {
        self.mark_read(network.into(), target.into(), time)
    }
    fn set_text_logs(&self, folder: Option<String>) {
        self.set_text_logs(folder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    LoggedOut,
    LoggingIn,
    Online,
    /// Couldn't start with the saved login (e.g. Keycloak unreachable).
    Failed(String),
}

/// A dialog to show over the main window.
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    JoinChannel {
        network: String,
    },
    BrowseChannels {
        network: String,
    },
    NewConversation {
        network: String,
    },
    Whois {
        network: String,
        info: Box<WhoisInfo>,
    },
    Search {
        network: String,
    },
    ChannelSettings {
        network: String,
        channel: String,
    },
}

/// Something the front end does once.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A desktop notification for a message in a buffer not in view.
    Notify {
        buffer: BufferId,
        message: ChatMessage,
    },
    OpenDialog(Dialog),
    /// Scroll a buffer's view to a message (its `stable_id`) and flash it.
    ScrollTo {
        buffer: BufferId,
        message: String,
    },
}

/// What changed since the front end last looked.
#[derive(Debug, Default)]
pub struct Changes {
    pub phase: bool,
    /// The control connection's state, or a network's.
    pub connection: bool,
    /// Networks or buffers came, went or were renamed: the sidebar's shape.
    pub structure: bool,
    /// Buffers whose messages changed.
    pub messages: BTreeSet<BufferId>,
    /// Buffers whose topic, members, typing, presence, read marker or
    /// unread counts changed.
    pub buffers: BTreeSet<BufferId>,
    /// Tabs, panes, or what they show.
    pub layout: bool,
    pub channel_lists: BTreeSet<String>,
    pub search: bool,
    pub channel_settings: bool,
    pub notice: bool,
    pub preferences: bool,
    pub servers: bool,
    pub effects: Vec<Effect>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        !(self.phase
            || self.connection
            || self.structure
            || self.layout
            || self.search
            || self.channel_settings
            || self.notice
            || self.preferences
            || self.servers)
            && self.messages.is_empty()
            && self.buffers.is_empty()
            && self.channel_lists.is_empty()
            && self.effects.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelList {
    pub channels: Vec<ChannelListing>,
    pub loading: bool,
}

/// The latest search and its results.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchState {
    pub network: String,
    pub text: String,
    /// Buffers (case-folded) whose results are still to come.
    pub pending: BTreeSet<String>,
    /// Newest first.
    pub hits: Vec<SearchHit>,
    pub errors: Vec<String>,
}

/// What the channel settings dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSettings {
    pub network: String,
    pub channel: String,
    pub modes: Option<String>,
    pub params: Vec<String>,
    pub bans: Option<Vec<BanEntry>>,
}

/// Saved across launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionState {
    tabs: Vec<WorkspaceTab>,
    current: Uuid,
    #[serde(default)]
    standalone_connected: bool,
}

pub const HISTORY_PAGE: u32 = 100;

pub struct AppState {
    pub phase: Phase,
    pub servers: ServerList,
    /// The control connection's state.
    pub control: Option<ConnectionState>,
    pub networks: Vec<NetworkState>,
    pub buffers: HashMap<BufferId, BufferModel>,
    tabs: Vec<WorkspaceTab>,
    current_tab: Uuid,
    /// The latest error worth showing; the front end clears it.
    pub notice: Option<String>,
    pub channel_lists: HashMap<String, ChannelList>,
    pub search: Option<SearchState>,
    pub channel_settings: Option<ChannelSettings>,
    pub prefs: Preferences,
    /// Whether the window is active; buffers are only marked read then.
    is_active: bool,
    /// Whether "No bouncer" was connected (reconnect at launch).
    pub standalone_connected: bool,
    /// Most recently viewed first; where to go when the open buffer goes.
    recent: Vec<BufferId>,
    /// A channel the user just joined; selected once it exists.
    pending_selection: Option<BufferId>,
    pages_to_target: u32,
    client: Option<Arc<dyn Client>>,
    paths: Option<Paths>,
    changes: Changes,
}

impl AppState {
    /// Loads settings, servers and tabs from `paths` (`None`: nothing is
    /// read or saved).
    pub fn new(paths: Option<Paths>) -> Self {
        let (prefs, servers, session) = match &paths {
            Some(p) => (
                Preferences::load(&p.preferences()),
                ServerList::load(&p.servers()),
                storage::read_json::<SessionState>(&p.session_state()),
            ),
            None => (Preferences::default(), ServerList::default(), None),
        };
        let session = session.filter(|s| !s.tabs.is_empty());
        let (tabs, current_tab, standalone_connected) = match session {
            Some(s) => {
                let current = if s.tabs.iter().any(|t| t.id == s.current) {
                    s.current
                } else {
                    s.tabs[0].id
                };
                (s.tabs, current, s.standalone_connected)
            }
            None => {
                let tab = WorkspaceTab::new(PaneLayout::default());
                let id = tab.id;
                (vec![tab], id, false)
            }
        };
        AppState {
            phase: Phase::LoggedOut,
            servers,
            control: None,
            networks: Vec::new(),
            buffers: HashMap::new(),
            tabs,
            current_tab,
            notice: None,
            channel_lists: HashMap::new(),
            search: None,
            channel_settings: None,
            prefs,
            is_active: true,
            standalone_connected,
            recent: Vec::new(),
            pending_selection: None,
            pages_to_target: 0,
            client: None,
            paths,
            changes: Changes::default(),
        }
    }

    pub fn paths(&self) -> Option<&Paths> {
        self.paths.as_ref()
    }

    /// What changed since the last call.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    // MARK: Settings

    /// The server in use; `None` only if the list is somehow empty.
    pub fn settings(&self) -> ServerSettings {
        self.servers
            .current()
            .cloned()
            .unwrap_or_else(ServerSettings::standalone)
    }

    pub fn is_standalone(&self) -> bool {
        self.settings().is_standalone()
    }

    pub fn save_servers(&mut self) {
        if let Some(p) = &self.paths {
            self.servers.save_to(&p.servers());
        }
        self.changes.servers = true;
    }

    /// Saves the preferences and refreshes what depends on them.
    pub fn preferences_changed(&mut self) {
        if let Some(p) = &self.paths {
            self.prefs.save(&p.preferences());
        }
        self.refresh_mentions(None);
        self.changes.preferences = true;
    }

    /// Where standalone mode writes text logs; `None` when they're off.
    pub fn text_log_path(&self) -> Option<String> {
        let default = self.paths.as_ref()?.default_text_logs();
        self.prefs.active_text_log_path(&default)
    }

    /// Starts or stops the plain-text logs of a running standalone session.
    pub fn apply_text_logs(&self) {
        if self.is_standalone()
            && let Some(client) = &self.client
        {
            client.set_text_logs(self.text_log_path());
        }
    }

    fn save_session(&self) {
        if let Some(p) = &self.paths {
            storage::write_json(
                &p.session_state(),
                &SessionState {
                    tabs: self.tabs.clone(),
                    current: self.current_tab,
                    standalone_connected: self.standalone_connected,
                },
            );
        }
    }

    pub fn set_standalone_connected(&mut self, connected: bool) {
        self.standalone_connected = connected;
        self.save_session();
    }

    // MARK: Lifecycle (the front end drives the async parts)

    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase != phase {
            self.phase = phase;
            self.changes.phase = true;
        }
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
        self.changes.notice = true;
    }

    /// Replaces the client (or drops it) and forgets the chat.
    pub fn set_client(&mut self, client: Option<Arc<dyn Client>>) {
        self.client = client;
        self.reset_chat();
    }

    pub fn has_client(&self) -> bool {
        self.client.is_some()
    }

    /// Before `connect`: a fresh chat, connecting.
    pub fn begin_connect(&mut self) {
        self.reset_chat();
        self.control = Some(ConnectionState::Connecting);
        self.changes.connection = true;
        self.set_phase(Phase::Online);
    }

    /// `connect` failed.
    pub fn connect_failed(&mut self, error: &BackroomError) {
        match error {
            BackroomError::NotLoggedIn { .. } => self.set_phase(Phase::LoggedOut),
            other => self.set_phase(Phase::Failed(describe(other))),
        }
    }

    pub fn logged_out(&mut self) {
        self.reset_chat();
        self.set_phase(Phase::LoggedOut);
    }

    pub fn reset_chat(&mut self) {
        self.control = None;
        self.networks.clear();
        self.buffers.clear();
        self.recent.clear();
        self.channel_lists.clear();
        self.search = None;
        self.channel_settings = None;
        self.changes.structure = true;
        self.changes.connection = true;
        self.changes.layout = true;
    }

    /// The window became active or inactive.
    pub fn set_active(&mut self, active: bool) {
        if self.is_active == active {
            return;
        }
        self.is_active = active;
        if active {
            for id in self.layout().visible_buffers() {
                // Coming back: mark where the unseen messages start.
                if let Some(b) = self.buffers.get_mut(&id)
                    && b.unread() > 0
                {
                    b.unread_line = b.read_marker;
                    self.changes.messages.insert(id.clone());
                }
                self.mark_read(&id);
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    // MARK: Events

    pub fn apply(&mut self, event: SessionEvent) {
        use SessionEvent as E;
        match event {
            E::Connection {
                network: None,
                state,
            } => {
                self.changes.connection = true;
                if let ConnectionState::AuthFailed { reason } = &state {
                    let notice = login_rejected(reason, self.settings().login);
                    self.reset_chat();
                    self.set_phase(Phase::LoggedOut);
                    self.set_notice(Some(notice));
                } else {
                    self.control = Some(state);
                }
            }
            E::Connection {
                network: Some(id),
                state,
            } => {
                if let Some(n) = self.network_mut(&id) {
                    n.connection = state;
                    self.changes.connection = true;
                }
            }
            E::NetworkUpdated { network } => {
                match self.network_mut(&network.id) {
                    Some(n) => n.network = network,
                    None => self.networks.push(NetworkState::new(network)),
                }
                self.changes.structure = true;
            }
            E::NetworkRemoved { id } => {
                self.networks.retain(|n| n.id() != id);
                self.buffers.retain(|b, _| b.network != id);
                self.recent.retain(|b| b.network != id);
                for tab in &mut self.tabs {
                    let gone: Vec<Uuid> = tab
                        .layout
                        .panes()
                        .iter()
                        .filter(|p| p.buffer.as_ref().is_some_and(|b| b.network == id))
                        .map(|p| p.id)
                        .collect();
                    for pane in gone {
                        tab.layout.show(None, pane);
                    }
                }
                self.changes.structure = true;
                self.layout_changed();
            }
            E::BufferAdded {
                network,
                name,
                kind,
            } => {
                self.ensure_buffer(&network, &name, Some(kind));
            }
            E::BufferRemoved { network, name } => self.buffer_removed(&network, &name),
            E::Topic {
                network,
                channel,
                topic,
            } => {
                let id = BufferId::new(network, &channel);
                if let Some(b) = self.buffers.get_mut(&id) {
                    b.topic = topic;
                    self.changes.buffers.insert(id);
                }
            }
            E::Members {
                network,
                channel,
                members,
            } => {
                let id = BufferId::new(network, &channel);
                if let Some(b) = self.buffers.get_mut(&id) {
                    b.set_members(members);
                    self.changes.buffers.insert(id.clone());
                    // Nick colours depend on ranks.
                    self.changes.messages.insert(id);
                }
            }
            E::Message {
                network,
                buffer,
                message,
            } => self.live_message(&network, &buffer, message),
            // History and read markers only update buffers that still
            // exist: they can arrive after we left a channel.
            E::History {
                network,
                buffer,
                messages,
            } => {
                let id = BufferId::new(network, &buffer);
                let count = messages.len();
                let Some(b) = self.buffers.get_mut(&id) else {
                    return;
                };
                b.merge(messages);
                b.history_loading = false;
                if count < HISTORY_PAGE as usize {
                    b.history_exhausted = true;
                }
                self.changes.messages.insert(id.clone());
                self.changes.buffers.insert(id.clone());
                if self.layout().visible_buffers().contains(&id) {
                    self.mark_read(&id);
                }
                self.load_towards_target(&id);
            }
            E::ReadMarker {
                network,
                buffer,
                time,
            } => {
                // Markers only move forward: soju's reply to the initial
                // query can arrive after this device marked the buffer read.
                let id = BufferId::new(network, &buffer);
                if let Some(b) = self.buffers.get_mut(&id)
                    && let Some(time) = time
                    && b.read_marker.is_none_or(|m| time > m)
                {
                    b.read_marker = Some(time);
                    self.changes.buffers.insert(id);
                }
            }
            E::ChannelList {
                network,
                channels,
                done,
            } => {
                // Chunks only extend a list someone asked for.
                if let Some(list) = self.channel_lists.get_mut(&network) {
                    list.channels.extend(channels);
                    if done {
                        list.loading = false;
                    }
                    self.changes.channel_lists.insert(network);
                }
            }
            E::SearchResults {
                network,
                target,
                text,
                results,
                error,
            } => {
                let Some(state) = &mut self.search else {
                    return;
                };
                if state.network != network
                    || state.text != text
                    || !state.pending.remove(&target.to_lowercase())
                {
                    return;
                }
                state.hits.extend(results);
                state
                    .hits
                    .sort_by(|a, b| b.message.time.cmp(&a.message.time));
                if let Some(error) = error {
                    state.errors.push(format!("{target}: {error}"));
                }
                self.changes.search = true;
            }
            E::Typing {
                network,
                buffer,
                nick,
                state,
            } => {
                let id = BufferId::new(network, &buffer);
                if let Some(b) = self.buffers.get_mut(&id) {
                    b.typing.insert(
                        nick.to_lowercase(),
                        Typing {
                            nick,
                            state,
                            since: SystemTime::now(),
                        },
                    );
                    self.changes.buffers.insert(id);
                }
            }
            E::Presence {
                network,
                nick,
                online,
            } => {
                let id = BufferId::new(network, &nick);
                if let Some(b) = self.buffers.get_mut(&id) {
                    b.online = Some(online);
                    self.changes.buffers.insert(id);
                }
            }
            E::ChannelModes {
                network,
                channel,
                modes,
                params,
            } => {
                if let Some(s) = self.channel_settings_for(&network, &channel) {
                    s.modes = Some(modes);
                    s.params = params;
                    self.changes.channel_settings = true;
                }
            }
            E::BanList {
                network,
                channel,
                bans,
            } => {
                if let Some(s) = self.channel_settings_for(&network, &channel) {
                    s.bans = Some(bans);
                    self.changes.channel_settings = true;
                }
            }
            E::OwnNick { network, nick } => {
                if let Some(n) = self.network_mut(&network) {
                    n.nick = Some(nick);
                    self.changes.connection = true;
                    self.refresh_mentions(Some(&network));
                }
            }
            E::Away { network, away } => {
                if let Some(n) = self.network_mut(&network) {
                    n.away = away;
                    self.changes.connection = true;
                }
            }
            E::Whois { network, info } => {
                self.changes.effects.push(Effect::OpenDialog(Dialog::Whois {
                    network,
                    info: Box::new(info),
                }));
            }
            E::Error { message, .. } => self.set_notice(Some(message)),
            E::Stopped => {
                self.control = None;
                self.changes.connection = true;
            }
        }
    }

    fn channel_settings_for(
        &mut self,
        network: &str,
        channel: &str,
    ) -> Option<&mut ChannelSettings> {
        self.channel_settings
            .as_mut()
            .filter(|s| s.network == network && s.channel.to_lowercase() == channel.to_lowercase())
    }

    fn live_message(&mut self, network: &str, name: &str, message: ChatMessage) {
        let id = self.ensure_buffer(network, name, None);
        let visible = self.layout().visible_buffers().contains(&id);
        let in_view = self.is_active && visible;
        let notify = !in_view && self.should_notify(&id, &message);
        let Some(b) = self.buffers.get_mut(&id) else {
            return;
        };
        // A message ends its sender's typing.
        b.typing.remove(&message.sender.to_lowercase());
        b.merge(vec![message.clone()]);
        self.changes.messages.insert(id.clone());
        self.changes.buffers.insert(id.clone());
        if visible {
            self.mark_read(&id);
        }
        if notify {
            self.changes.effects.push(Effect::Notify {
                buffer: id,
                message,
            });
        }
    }

    fn buffer_removed(&mut self, network: &str, name: &str) {
        let id = BufferId::new(network, name);
        self.buffers.remove(&id);
        self.recent.retain(|b| b != &id);
        if let Some(n) = self.network_mut(network) {
            n.buffers.retain(|b| b != &id);
        }
        // Panes showing it go back to the previous conversation, else the
        // network's server buffer.
        let fallback = self
            .recent
            .iter()
            .find(|b| self.buffers.contains_key(b))
            .cloned()
            .or_else(|| {
                self.network(network)?
                    .buffers
                    .iter()
                    .find(|b| {
                        self.buffers
                            .get(b)
                            .is_some_and(|m| m.kind == BufferKind::Server)
                    })
                    .cloned()
            });
        for tab in &mut self.tabs {
            let showing: Vec<Uuid> = tab
                .layout
                .panes()
                .iter()
                .filter(|p| p.buffer.as_ref() == Some(&id))
                .map(|p| p.id)
                .collect();
            for pane in showing {
                tab.layout.show(fallback.clone(), pane);
            }
        }
        self.changes.structure = true;
        self.layout_changed();
        if let Some(f) = fallback
            && self.layout().visible_buffers().contains(&f)
        {
            self.activate(&f);
        }
    }

    /// The buffer for `name`, made if it's new.
    pub fn ensure_buffer(
        &mut self,
        network: &str,
        name: &str,
        kind: Option<BufferKind>,
    ) -> BufferId {
        let id = BufferId::new(network, name);
        if let Some(b) = self.buffers.get_mut(&id) {
            if let Some(kind) = kind
                && b.kind != kind
            {
                b.kind = kind;
                self.changes.structure = true;
            }
            return id;
        }
        let kind = kind.unwrap_or_else(|| guess_kind(name));
        let mut buffer = BufferModel::new(id.clone(), name, kind);
        buffer.mentions = self.mention_matcher(network);
        self.buffers.insert(id.clone(), buffer);
        if let Some(n) = self.network_mut(network) {
            // The server buffer goes first.
            if kind == BufferKind::Server {
                n.buffers.insert(0, id.clone());
            } else {
                n.buffers.push(id.clone());
            }
        }
        self.changes.structure = true;
        if self.pending_selection.as_ref() == Some(&id) {
            self.pending_selection = None;
            self.select(Some(id.clone()));
        } else if self.layout().visible_buffers().contains(&id) {
            // A pane from last time shows it again.
            self.changes.layout = true;
            self.activate(&id);
        }
        id
    }

    pub fn network(&self, id: &str) -> Option<&NetworkState> {
        self.networks.iter().find(|n| n.id() == id)
    }

    fn network_mut(&mut self, id: &str) -> Option<&mut NetworkState> {
        self.networks.iter_mut().find(|n| n.id() == id)
    }

    pub fn network_name(&self, id: &str) -> String {
        self.network(id)
            .map_or_else(|| id.to_owned(), |n| n.network.name.clone())
    }

    /// A buffer's title: the network's name for its server buffer.
    pub fn buffer_title(&self, id: &BufferId) -> String {
        match self.buffers.get(id) {
            Some(b) if b.kind == BufferKind::Server => self.network_name(&id.network),
            Some(b) => b.name.clone(),
            None => id.key.clone(),
        }
    }

    // MARK: Tabs and panes

    pub fn tabs(&self) -> &[WorkspaceTab] {
        &self.tabs
    }

    pub fn current_tab(&self) -> Uuid {
        self.current_tab
    }

    fn tab_index(&self) -> usize {
        self.tabs
            .iter()
            .position(|t| t.id == self.current_tab)
            .unwrap_or(0)
    }

    /// The current tab's panes.
    pub fn layout(&self) -> &PaneLayout {
        &self.tabs[self.tab_index()].layout
    }

    fn layout_mut(&mut self) -> &mut PaneLayout {
        let i = self.tab_index();
        &mut self.tabs[i].layout
    }

    fn layout_changed(&mut self) {
        self.changes.layout = true;
        self.save_session();
    }

    /// The focused pane's buffer: what the member list and window title follow.
    pub fn selection(&self) -> Option<BufferId> {
        self.layout().focused_pane()?.buffer.clone()
    }

    pub fn selected_buffer(&self) -> Option<&BufferModel> {
        self.buffers.get(&self.selection()?)
    }

    /// Shows a buffer in the focused pane.
    pub fn select(&mut self, id: Option<BufferId>) {
        let focused = self.layout().focused;
        if self.layout().focused_pane().map(|p| &p.buffer) == Some(&id) {
            if let Some(id) = id {
                self.activate(&id);
            }
            return;
        }
        self.layout_mut().show(id.clone(), focused);
        self.layout_changed();
        if let Some(id) = id {
            self.activate(&id);
        }
    }

    /// A buffer came into view: load its history and mark it read.
    fn activate(&mut self, id: &BufferId) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(b) = self.buffers.get_mut(id) else {
            return;
        };
        // Before marking read: the line shows where this visit's news starts.
        b.unread_line = b.read_marker;
        let load = !b.history_requested;
        if load {
            b.history_requested = true;
            b.history_loading = true;
        }
        let name = b.name.clone();
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.clone());
        self.changes.messages.insert(id.clone());
        if load {
            self.run(|| client.load_history(&id.network, &name, None, HISTORY_PAGE));
        }
        self.mark_read(id);
    }

    /// A new tab showing the current conversation, after the current tab.
    pub fn new_tab(&mut self) {
        let tab = WorkspaceTab::new(PaneLayout::new(self.selection()));
        let id = tab.id;
        let i = self.tab_index();
        self.tabs.insert(i + 1, tab);
        self.show_tab(id);
    }

    pub fn show_tab(&mut self, id: Uuid) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        self.current_tab = id;
        self.layout_changed();
        // Its panes come into view: load and mark read.
        for b in self.layout().visible_buffers() {
            self.activate(&b);
        }
    }

    /// The tab `offset` places away from the current one, wrapping around.
    pub fn cycle_tab(&mut self, offset: isize) {
        let n = self.tabs.len() as isize;
        let i = self.tab_index() as isize;
        let next = self.tabs[((i + offset) % n + n) as usize % n as usize].id;
        self.show_tab(next);
    }

    /// Tab number `n` (from 1); 9 is the last tab.
    pub fn show_tab_number(&mut self, n: usize) {
        let last = self.tabs.len() - 1;
        let i = if n == 9 {
            last
        } else {
            (n.max(1) - 1).min(last)
        };
        self.show_tab(self.tabs[i].id);
    }

    /// Closes a tab (never the last one); its neighbour is shown.
    pub fn close_tab(&mut self, id: Uuid) {
        if self.tabs.len() < 2 {
            return;
        }
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let was_current = id == self.current_tab;
        self.tabs.remove(i);
        if was_current {
            let next = self.tabs[i.min(self.tabs.len() - 1)].id;
            self.show_tab(next);
        } else {
            self.layout_changed();
        }
    }

    pub fn rename_tab(&mut self, id: Uuid, name: &str) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            let name = name.trim();
            t.name = (!name.is_empty()).then(|| name.to_owned());
            self.layout_changed();
        }
    }

    /// Moves a tab to position `to`.
    pub fn move_tab(&mut self, id: Uuid, to: usize) {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = self.tabs.remove(from);
        self.tabs.insert(to.min(self.tabs.len()), tab);
        self.layout_changed();
    }

    /// The tab's name, or its conversations' names.
    pub fn tab_title(&self, tab: &WorkspaceTab) -> String {
        if let Some(name) = &tab.name {
            return name.clone();
        }
        let mut seen = std::collections::HashSet::new();
        let names: Vec<String> = tab
            .layout
            .panes()
            .iter()
            .filter_map(|p| p.buffer.as_ref())
            .map(|b| self.buffer_title(b))
            .filter(|n| seen.insert(n.to_lowercase()))
            .collect();
        if names.is_empty() {
            "Empty".into()
        } else {
            names.join(", ")
        }
    }

    /// Unread and mention counts for a tab's conversations.
    pub fn tab_unread(&self, tab: &WorkspaceTab) -> (usize, usize) {
        tab.layout
            .visible_buffers()
            .iter()
            .filter_map(|b| self.buffers.get(b))
            .fold((0, 0), |(a, m), b| {
                let (ba, bm) = b.unread_counts();
                (a + ba, m + bm)
            })
    }

    pub fn focus(&mut self, pane: Uuid) {
        if self.layout().focused != pane && self.layout().pane(pane).is_some() {
            self.layout_mut().focused = pane;
            self.layout_changed();
        }
    }

    /// Splits the focused pane; the new pane shows `buffer` (by default
    /// the same buffer) and gets the focus.
    pub fn split(&mut self, axis: SplitAxis, buffer: Option<BufferId>) {
        let shown = buffer.or_else(|| self.selection());
        let focused = self.layout().focused;
        self.layout_mut().split(focused, axis, shown.clone());
        self.layout_changed();
        if let Some(b) = shown {
            self.activate(&b);
        }
    }

    /// Closes a pane (the focused one by default); with one pane left, the tab.
    pub fn close_pane(&mut self, pane: Option<Uuid>) {
        if self.layout().panes().len() == 1 {
            self.close_tab(self.current_tab);
        } else {
            let pane = pane.unwrap_or(self.layout().focused);
            self.layout_mut().close(pane);
            self.layout_changed();
        }
    }

    pub fn can_close_pane(&self) -> bool {
        self.layout().panes().len() > 1 || self.tabs.len() > 1
    }

    pub fn close_other_panes(&mut self) {
        self.layout_mut().close_others();
        self.layout_changed();
    }

    pub fn move_focus(&mut self, direction: PaneDirection) {
        if let Some(next) = self.layout().neighbour(direction) {
            self.layout_mut().focused = next;
            self.layout_changed();
        }
    }

    /// The user dragged a divider. Saved, but nothing to redraw.
    pub fn resize_split(&mut self, split: Uuid, fractions: Vec<f64>) {
        self.layout_mut().resize(split, fractions);
        self.save_session();
    }

    /// Opens (or switches to) a private conversation with `nick`.
    pub fn open_query(&mut self, network: &str, nick: &str) {
        let id = self.ensure_buffer(network, nick, Some(BufferKind::Query));
        self.select(Some(id));
    }

    /// Brings a buffer into view (from a notification): in a pane of the
    /// current tab if one shows it, else in the focused pane.
    pub fn reveal(&mut self, id: &BufferId) {
        if !self.buffers.contains_key(id) {
            return;
        }
        let pane = self
            .layout()
            .panes()
            .iter()
            .find(|p| p.buffer.as_ref() == Some(id))
            .map(|p| p.id);
        match pane {
            Some(pane) => {
                self.focus(pane);
                self.activate(id);
            }
            None => self.select(Some(id.clone())),
        }
    }

    /// Shows a message from search: selects its buffer and scrolls to it,
    /// loading older history until it's there (up to 20 pages).
    pub fn reveal_message(&mut self, network: &str, buffer: &str, message: &ChatMessage) {
        let id = self.ensure_buffer(network, buffer, None);
        self.reveal(&id);
        if let Some(b) = self.buffers.get_mut(&id) {
            b.scroll_target = Some(stable_id(message));
        }
        self.pages_to_target = 20;
        self.load_towards_target(&id);
    }

    fn load_towards_target(&mut self, id: &BufferId) {
        let Some(b) = self.buffers.get_mut(id) else {
            return;
        };
        let Some(target) = b.scroll_target.clone() else {
            return;
        };
        if b.messages().iter().any(|m| stable_id(m) == target) {
            b.scroll_target = None;
            self.changes.effects.push(Effect::ScrollTo {
                buffer: id.clone(),
                message: target,
            });
            return;
        }
        if b.history_exhausted {
            b.scroll_target = None;
            return;
        }
        if self.pages_to_target == 0 {
            b.scroll_target = None;
            self.set_notice(Some("That message is too far back to show here.".into()));
            return;
        }
        self.pages_to_target -= 1;
        if b.history_requested {
            self.load_older(id);
        }
    }

    // MARK: Notifications

    fn should_notify(&self, id: &BufferId, message: &ChatMessage) -> bool {
        let Some(b) = self.buffers.get(id) else {
            return false;
        };
        if !self.prefs.notifications || message.own || b.mentions.is_ignored(&message.sender) {
            return false;
        }
        if b.is_mention(message) {
            return true;
        }
        match b.kind {
            BufferKind::Query => is_chat(message.kind),
            BufferKind::Channel => self.prefs.notify_all_messages && is_chat(message.kind),
            BufferKind::Server => false,
        }
    }

    /// Unread mentions and private messages everywhere, for the launcher badge.
    pub fn total_mentions(&self) -> usize {
        self.buffers
            .values()
            .map(BufferModel::unread_mentions)
            .sum()
    }

    // MARK: Mentions and ignores

    fn mention_matcher(&self, network: &str) -> MentionMatcher {
        MentionMatcher::new(
            self.network(network).and_then(|n| n.nick.as_deref()),
            &self.prefs.highlight_words,
            &self.prefs.ignored,
        )
    }

    /// Rebuilds mention matching after our nick, the highlight words or the
    /// ignore list changed (`None`: everywhere).
    pub fn refresh_mentions(&mut self, network: Option<&str>) {
        let ids: Vec<BufferId> = self
            .buffers
            .keys()
            .filter(|id| network.is_none_or(|n| id.network == n))
            .cloned()
            .collect();
        for id in ids {
            let matcher = self.mention_matcher(&id.network);
            if let Some(b) = self.buffers.get_mut(&id)
                && b.mentions != matcher
            {
                b.mentions = matcher;
                self.changes.messages.insert(id.clone());
                self.changes.buffers.insert(id);
            }
        }
    }

    // MARK: Search and channel settings

    /// Searches one buffer, or every conversation on the network (`None`).
    pub fn start_search(&mut self, network: &str, target: Option<&str>, text: &str) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let targets: Vec<String> = match target {
            Some(t) => vec![t.to_owned()],
            None => self
                .network(network)
                .map(|n| {
                    n.buffers
                        .iter()
                        .filter_map(|b| self.buffers.get(b))
                        .filter(|b| b.kind != BufferKind::Server)
                        .map(|b| b.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
        };
        self.search = Some(SearchState {
            network: network.to_owned(),
            text: text.to_owned(),
            pending: targets.iter().map(|t| t.to_lowercase()).collect(),
            hits: Vec::new(),
            errors: Vec::new(),
        });
        self.changes.search = true;
        for t in targets {
            self.run(|| client.search(network, &t, text));
        }
    }

    pub fn open_channel_settings(&mut self, network: &str, channel: &str) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.channel_settings = Some(ChannelSettings {
            network: network.to_owned(),
            channel: channel.to_owned(),
            modes: None,
            params: Vec::new(),
            bans: None,
        });
        self.changes.channel_settings = true;
        self.open(Dialog::ChannelSettings {
            network: network.to_owned(),
            channel: channel.to_owned(),
        });
        self.run(|| client.load_channel_settings(network, channel));
    }

    pub fn open(&mut self, dialog: Dialog) {
        self.changes.effects.push(Effect::OpenDialog(dialog));
    }

    /// Typing state for a buffer's conversation (not the server buffer).
    pub fn set_typing(&self, id: &BufferId, state: TypingState) {
        if let (Some(client), Some(b)) = (&self.client, self.buffers.get(id))
            && b.kind != BufferKind::Server
        {
            let _ = client.set_typing(&id.network, &b.name, state);
        }
    }

    // MARK: IRC actions

    /// Sends a raw line on a network.
    pub fn send_raw(&mut self, network: &str, line: &str) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        self.run(|| client.send_raw(network, line))
    }

    pub fn set_away(&mut self, network: &str, away: bool) {
        self.send_raw(network, if away { "AWAY :Away" } else { "AWAY" });
    }

    /// Our nick on a network, once known.
    pub fn own_nick(&self, network: &str) -> Option<&str> {
        self.network(network)?.nick.as_deref()
    }

    /// Our membership prefix in a channel ("" if none, `None` if unknown).
    pub fn own_prefix<'a>(&self, buffer: &'a BufferModel) -> Option<&'a str> {
        buffer.prefix(self.own_nick(&buffer.id.network)?)
    }

    /// The network menu commands act on: the selected buffer's, else the first.
    pub fn current_network(&self) -> Option<String> {
        self.selection()
            .map(|b| b.network)
            .or_else(|| self.networks.first().map(|n| n.id().to_owned()))
    }

    /// Joins channels (comma-separated) and switches to the first: now if
    /// we're already in it, else once the server confirms.
    pub fn join(&mut self, network: &str, channel: &str, key: Option<&str>) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        let first = channel.split(',').next().unwrap_or_default();
        let target = BufferId::new(network, first);
        if self.buffers.contains_key(&target) {
            self.select(Some(target));
        } else {
            self.pending_selection = Some(target);
        }
        self.run(|| client.join(network, channel, key))
    }

    /// Asks for a network's channel list, replacing any earlier one.
    pub fn list_channels(&mut self, network: &str) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.channel_lists.insert(
            network.to_owned(),
            ChannelList {
                channels: Vec::new(),
                loading: true,
            },
        );
        self.changes.channel_lists.insert(network.to_owned());
        if !self.run(|| client.list_channels(network))
            && let Some(l) = self.channel_lists.get_mut(network)
        {
            l.loading = false;
        }
    }

    /// What removing a network deletes, for the confirmation.
    pub fn remove_network_message(&self) -> &'static str {
        if self.is_standalone() {
            "Backroom disconnects from it and deletes its channels and history on this computer."
        } else {
            "soju disconnects from it and deletes its channels and history."
        }
    }

    /// Asks the session to add a network; it appears once it exists.
    pub fn add_network(&mut self, fields: &NetworkFields) {
        if let Some(client) = self.client.clone() {
            self.run(|| client.add_network(fields.attributes(false)));
        }
    }

    /// Changes a network's settings; it reconnects.
    pub fn change_network(&mut self, id: &str, fields: &NetworkFields) {
        if let Some(client) = self.client.clone() {
            // Empty values clear attributes, e.g. a removed nick.
            self.run(|| client.change_network(id, fields.attributes(true)));
        }
    }

    pub fn remove_network(&mut self, id: &str) {
        if let Some(client) = self.client.clone() {
            self.run(|| client.remove_network(id));
        }
    }

    /// Leaves a channel or closes a private conversation.
    pub fn close(&mut self, id: &BufferId) {
        let (Some(client), Some(b)) = (self.client.clone(), self.buffers.get(id)) else {
            return;
        };
        let name = b.name.clone();
        self.run(|| client.close_buffer(&id.network, &name));
    }

    /// Asks for the page before the oldest loaded message.
    pub fn load_older(&mut self, id: &BufferId) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(b) = self.buffers.get_mut(id) else {
            return;
        };
        if b.history_loading || b.history_exhausted || b.cleared {
            return;
        }
        let Some(oldest) = b.messages().first().map(|m| m.time) else {
            return;
        };
        b.history_loading = true;
        let name = b.name.clone();
        self.changes.buffers.insert(id.clone());
        self.run(|| client.load_history(&id.network, &name, Some(oldest), HISTORY_PAGE));
    }

    /// Moves the read marker to the newest message, here and on the server.
    pub fn mark_read(&mut self, id: &BufferId) {
        if !self.is_active {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(b) = self.buffers.get_mut(id) else {
            return;
        };
        let Some(newest) = b.messages().last().map(|m| m.time) else {
            return;
        };
        if b.read_marker.is_some_and(|m| m >= newest) {
            return;
        }
        b.read_marker = Some(newest);
        let name = b.name.clone();
        self.changes.buffers.insert(id.clone());
        self.run(|| client.mark_read(&id.network, &name, newest));
    }

    /// Sends a composer line to a buffer. Returns true if it was handled
    /// and the composer should be cleared.
    pub fn send(&mut self, id: &BufferId, input: &str) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        let Some(b) = self.buffers.get(id) else {
            return false;
        };
        let (name, kind) = (b.name.clone(), b.kind);
        let Some(action) = composer::parse(input, &name) else {
            return false;
        };
        let network = id.network.as_str();
        match action {
            ComposerAction::Message(text) | ComposerAction::Action(text)
                if kind == BufferKind::Server =>
            {
                let _ = text;
                self.set_notice(Some(
                    "Messages can't go to the server buffer. Use /join #channel.".into(),
                ));
                false
            }
            ComposerAction::Message(text) => {
                self.run(|| client.send_message(network, &name, &text))
            }
            ComposerAction::Action(text) => {
                self.run(|| client.send_message(network, &name, &composer::action_text(&text)))
            }
            ComposerAction::Join { channels, key } => self.join(network, &channels, key.as_deref()),
            ComposerAction::Part { channel, reason } => {
                self.run(|| client.part(network, &channel, reason.as_deref()))
            }
            ComposerAction::Query { nick, text } => {
                self.open_query(network, &nick);
                match text {
                    None => true,
                    Some(text) => self.run(|| client.send_message(network, &nick, &text)),
                }
            }
            ComposerAction::Raw(line) => self.run(|| client.send_raw(network, &line)),
            ComposerAction::Cycle { channel } => {
                self.pending_selection = Some(id.clone());
                self.run(|| {
                    client.part(network, &channel, None)?;
                    client.join(network, &channel, None)
                })
            }
            ComposerAction::ListChannels => {
                self.open(Dialog::BrowseChannels {
                    network: network.to_owned(),
                });
                true
            }
            ComposerAction::Clear => {
                if let Some(b) = self.buffers.get_mut(id) {
                    b.clear();
                }
                self.changes.messages.insert(id.clone());
                self.changes.buffers.insert(id.clone());
                true
            }
            ComposerAction::Close => {
                if kind == BufferKind::Server {
                    self.set_notice(Some("The server buffer can't be closed.".into()));
                    return false;
                }
                self.close(id);
                true
            }
            ComposerAction::Ignore(nick) => {
                self.prefs.ignore(&nick);
                self.preferences_changed();
                self.set_notice(Some(format!(
                    "Ignoring {nick}. /unignore {nick} undoes it."
                )));
                true
            }
            ComposerAction::Unignore(nick) => {
                self.prefs.unignore(&nick);
                self.preferences_changed();
                true
            }
            ComposerAction::Invalid(message) => {
                self.set_notice(Some(message));
                false
            }
        }
    }

    /// Records a line sent from a buffer's composer, for ↑/↓.
    pub fn remember_sent(&mut self, id: &BufferId, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        if let Some(b) = self.buffers.get_mut(id) {
            b.sent_history.push(line.to_owned());
            if b.sent_history.len() > 200 {
                b.sent_history.remove(0);
            }
        }
    }

    /// Nicks to complete in a buffer: recent speakers first, then everyone.
    pub fn completion_nicks(&self, id: &BufferId) -> Vec<String> {
        let Some(b) = self.buffers.get(id) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let speakers = b
            .messages()
            .iter()
            .rev()
            .filter(|m| !m.own && is_chat(m.kind))
            .map(|m| m.sender.clone());
        let members = b.members().iter().map(|m| m.nick.clone());
        let partner = (b.kind == BufferKind::Query).then(|| b.name.clone());
        speakers
            .chain(members)
            .chain(partner)
            .filter(|n| seen.insert(n.to_lowercase()))
            .collect()
    }

    /// Channels on a buffer's network, for completion.
    pub fn completion_channels(&self, network: &str) -> Vec<String> {
        self.network(network)
            .map(|n| {
                n.buffers
                    .iter()
                    .filter_map(|b| self.buffers.get(b))
                    .filter(|b| b.kind == BufferKind::Channel)
                    .map(|b| b.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a message is shown as one of ours, a mention or neither.
    pub fn message_is_mention(&self, id: &BufferId, m: &ChatMessage) -> bool {
        self.buffers.get(id).is_some_and(|b| b.is_mention(m))
    }

    fn run(&mut self, body: impl FnOnce() -> ClientResult) -> bool {
        match body() {
            Ok(()) => true,
            Err(e) => {
                self.set_notice(Some(describe(&e)));
                false
            }
        }
    }
}

fn guess_kind(name: &str) -> BufferKind {
    if name == "*" {
        BufferKind::Server
    } else if composer::is_channel(name) {
        BufferKind::Channel
    } else {
        BufferKind::Query
    }
}

/// What to say when the server turns the login down.
pub fn login_rejected(reason: &str, login: Login) -> String {
    // From the core when soju doesn't offer our SASL mechanism.
    if reason.contains("doesn't accept OAUTHBEARER") {
        return "This server doesn't support single sign-on. Check the server address, or log in with a username and password.".into();
    }
    if reason.contains("doesn't accept PLAIN") {
        return "This server doesn't accept passwords; log in with single sign-on.".into();
    }
    match login {
        // soju's 904 text is generic ("Authentication failed").
        Login::Password => "Wrong username or password.".into(),
        Login::SingleSignOn => format!("Your login was rejected ({reason}). Log in again."),
        Login::Standalone => reason.to_owned(),
    }
}

/// An error as a sentence for the user.
pub fn describe(error: &BackroomError) -> String {
    match error {
        BackroomError::NotLoggedIn { message }
        | BackroomError::Auth { message }
        | BackroomError::Config { message }
        | BackroomError::Store { message }
        | BackroomError::Internal { message } => message.clone(),
        BackroomError::NoLoginInProgress => "no login in progress".into(),
        BackroomError::NotConnected => "not connected".into(),
    }
}

/// Whether a kind is shown as an event line (joins, modes, …).
pub fn is_event(kind: MessageKind) -> bool {
    !matches!(
        kind,
        MessageKind::Privmsg
            | MessageKind::Notice
            | MessageKind::Action
            | MessageKind::Reply
            | MessageKind::Error
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use backroom_ffi::Network;

    use super::*;
    use crate::buffer::tests::msg;

    #[derive(Default)]
    struct FakeClient {
        calls: Mutex<Vec<String>>,
    }

    impl FakeClient {
        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
        fn record(&self, call: String) -> ClientResult {
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
    }

    impl Client for FakeClient {
        fn send_message(&self, n: &str, t: &str, text: &str) -> ClientResult {
            self.record(format!("send {n} {t} {text}"))
        }
        fn join(&self, n: &str, c: &str, k: Option<&str>) -> ClientResult {
            self.record(format!("join {n} {c} {k:?}"))
        }
        fn part(&self, n: &str, c: &str, _: Option<&str>) -> ClientResult {
            self.record(format!("part {n} {c}"))
        }
        fn search(&self, n: &str, t: &str, text: &str) -> ClientResult {
            self.record(format!("search {n} {t} {text}"))
        }
        fn set_typing(&self, _: &str, _: &str, _: TypingState) -> ClientResult {
            Ok(())
        }
        fn load_channel_settings(&self, n: &str, c: &str) -> ClientResult {
            self.record(format!("settings {n} {c}"))
        }
        fn list_channels(&self, n: &str) -> ClientResult {
            self.record(format!("list {n}"))
        }
        fn add_network(&self, _: HashMap<String, String>) -> ClientResult {
            self.record("add".into())
        }
        fn change_network(&self, n: &str, _: HashMap<String, String>) -> ClientResult {
            self.record(format!("change {n}"))
        }
        fn remove_network(&self, n: &str) -> ClientResult {
            self.record(format!("remove {n}"))
        }
        fn send_raw(&self, n: &str, line: &str) -> ClientResult {
            self.record(format!("raw {n} {line}"))
        }
        fn close_buffer(&self, n: &str, b: &str) -> ClientResult {
            self.record(format!("close {n} {b}"))
        }
        fn load_history(
            &self,
            n: &str,
            t: &str,
            before: Option<SystemTime>,
            _: u32,
        ) -> ClientResult {
            self.record(format!("history {n} {t} {}", before.is_some()))
        }
        fn mark_read(&self, n: &str, t: &str, _: SystemTime) -> ClientResult {
            self.record(format!("read {n} {t}"))
        }
        fn set_text_logs(&self, _: Option<String>) {}
    }

    fn online() -> (AppState, Arc<FakeClient>) {
        let mut app = AppState::new(None);
        let client = Arc::new(FakeClient::default());
        app.set_client(Some(client.clone()));
        app.begin_connect();
        app.apply(SessionEvent::NetworkUpdated {
            network: Network {
                id: "1".into(),
                name: "Libera".into(),
                upstream_state: None,
                attributes: HashMap::new(),
            },
        });
        app.apply(SessionEvent::OwnNick {
            network: "1".into(),
            nick: "dave".into(),
        });
        app.apply(SessionEvent::BufferAdded {
            network: "1".into(),
            name: "*".into(),
            kind: BufferKind::Server,
        });
        app.take_changes();
        (app, client)
    }

    fn message(app: &mut AppState, buffer: &str, m: ChatMessage) {
        app.apply(SessionEvent::Message {
            network: "1".into(),
            buffer: buffer.into(),
            message: m,
        });
    }

    #[test]
    fn joining_selects_the_channel_once_it_exists() {
        let (mut app, client) = online();
        let id = BufferId::new("1", "*");
        app.select(Some(id.clone()));
        assert!(app.send(&id, "/join backroom"));
        assert_eq!(
            client.calls()[..2],
            [
                "history 1 * false".to_owned(),
                "join 1 #backroom None".into()
            ]
        );
        assert_eq!(app.selection(), Some(id));
        app.apply(SessionEvent::BufferAdded {
            network: "1".into(),
            name: "#Backroom".into(),
            kind: BufferKind::Channel,
        });
        assert_eq!(app.selection(), Some(BufferId::new("1", "#backroom")));
        assert_eq!(client.calls(), ["history 1 #Backroom false"]);
        let changes = app.take_changes();
        assert!(changes.structure && changes.layout);
    }

    #[test]
    fn mentions_notify_when_not_in_view() {
        let (mut app, _client) = online();
        app.apply(SessionEvent::BufferAdded {
            network: "1".into(),
            name: "#a".into(),
            kind: BufferKind::Channel,
        });
        app.take_changes();
        message(&mut app, "#a", msg(1, "x", MessageKind::Privmsg, "hi all"));
        message(&mut app, "#a", msg(2, "x", MessageKind::Privmsg, "hi dave"));
        message(&mut app, "bob", msg(3, "bob", MessageKind::Privmsg, "psst"));
        let changes = app.take_changes();
        let notified: Vec<_> = changes
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::Notify { message, .. } => Some(message.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notified, ["hi dave", "psst"]);
        assert_eq!(
            app.buffers[&BufferId::new("1", "bob")].kind,
            BufferKind::Query
        );
        assert_eq!(app.total_mentions(), 2);
    }

    #[test]
    fn viewing_a_buffer_marks_it_read() {
        let (mut app, client) = online();
        let id = BufferId::new("1", "#a");
        app.apply(SessionEvent::BufferAdded {
            network: "1".into(),
            name: "#a".into(),
            kind: BufferKind::Channel,
        });
        app.select(Some(id.clone()));
        client.calls();
        message(&mut app, "#a", msg(5, "x", MessageKind::Privmsg, "hi"));
        assert_eq!(client.calls(), ["read 1 #a"]);
        assert_eq!(app.buffers[&id].unread(), 0);
        app.set_active(false);
        message(&mut app, "#a", msg(6, "x", MessageKind::Privmsg, "again"));
        assert!(client.calls().is_empty());
        assert_eq!(app.buffers[&id].unread(), 1);
        app.set_active(true);
        assert_eq!(client.calls(), ["read 1 #a"]);
        assert_eq!(
            app.buffers[&id].unread_line,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5))
        );
    }

    #[test]
    fn leaving_a_channel_falls_back_to_the_previous_buffer() {
        let (mut app, _client) = online();
        for name in ["#a", "#b"] {
            app.apply(SessionEvent::BufferAdded {
                network: "1".into(),
                name: name.into(),
                kind: BufferKind::Channel,
            });
        }
        app.select(Some(BufferId::new("1", "#a")));
        app.select(Some(BufferId::new("1", "#b")));
        app.apply(SessionEvent::BufferRemoved {
            network: "1".into(),
            name: "#b".into(),
        });
        assert_eq!(app.selection(), Some(BufferId::new("1", "#a")));
        // History for a buffer that's gone doesn't bring it back.
        app.apply(SessionEvent::History {
            network: "1".into(),
            buffer: "#b".into(),
            messages: vec![],
        });
        assert!(!app.buffers.contains_key(&BufferId::new("1", "#b")));
    }

    #[test]
    fn search_collects_results_from_every_conversation() {
        let (mut app, client) = online();
        for name in ["#a", "bob"] {
            app.ensure_buffer("1", name, None);
        }
        client.calls();
        app.start_search("1", None, " hello ");
        let mut calls = client.calls();
        calls.sort();
        assert_eq!(calls, ["search 1 #a hello", "search 1 bob hello"]);
        app.apply(SessionEvent::SearchResults {
            network: "1".into(),
            target: "#A".into(),
            text: "hello".into(),
            results: vec![SearchHit {
                buffer: "#a".into(),
                message: msg(1, "x", MessageKind::Privmsg, "hello"),
            }],
            error: None,
        });
        let search = app.search.as_ref().unwrap();
        assert_eq!(search.hits.len(), 1);
        assert_eq!(search.pending.len(), 1);
    }

    #[test]
    fn rejected_login_logs_out_with_a_reason() {
        let (mut app, _client) = online();
        app.servers.save(ServerSettings::e2e_password());
        app.servers.selected = Some(ServerSettings::e2e_password().id);
        app.apply(SessionEvent::Connection {
            network: None,
            state: ConnectionState::AuthFailed {
                reason: "904".into(),
            },
        });
        assert_eq!(app.phase, Phase::LoggedOut);
        assert_eq!(app.notice.as_deref(), Some("Wrong username or password."));
        assert!(app.networks.is_empty());
    }

    #[test]
    fn tabs_and_panes_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let mut app = AppState::new(Some(paths.clone()));
        app.select(Some(BufferId::new("1", "#a")));
        app.split(SplitAxis::Horizontal, Some(BufferId::new("1", "#b")));
        app.new_tab();
        app.rename_tab(app.current_tab(), "Work");
        let again = AppState::new(Some(paths));
        assert_eq!(again.tabs().len(), 2);
        assert_eq!(again.current_tab(), app.current_tab());
        assert_eq!(again.tab_title(&again.tabs()[1]), "Work");
        assert_eq!(again.tabs()[0].layout.panes().len(), 2);
    }

    #[test]
    fn composer_errors_become_notices() {
        let (mut app, _client) = online();
        let id = BufferId::new("1", "*");
        assert!(!app.send(&id, "hello"));
        assert!(app.notice.as_deref().unwrap().contains("server buffer"));
        assert!(!app.send(&id, "/kick bob"));
        assert!(app.send(&id, "/ignore Troll"));
        assert_eq!(app.prefs.ignored, ["Troll"]);
    }
}
