//! Owns the app state and the core client, runs the async parts (login,
//! connect, the event loop) on GLib's main loop, and asks the window to
//! redraw what changed.
//!
//! Every change to the state goes through [`Controller::act`], which never
//! nests: a signal fired while the window redraws is run right after.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use backroom_app::app::{AppState, Effect, Phase, describe};
use backroom_app::buffer::BufferId;
use backroom_app::servers::{CALLBACK_SCHEME, ServerSettings};
use backroom_app::storage::Paths;
use backroom_ffi::{BackroomClient, BackroomError, LinkPreviewer};
use gtk::{gio, glib};
use uuid::Uuid;

use crate::desktop::secrets::KeyringStore;
use crate::desktop::window::Window;

pub struct Controller {
    pub app: adw::Application,
    pub state: RefCell<AppState>,
    client: RefCell<Option<Arc<BackroomClient>>>,
    window: OnceCell<Window>,
    update_queued: Cell<bool>,
    /// Bumped whenever a session starts or stops; an event loop from an
    /// older session stops reading.
    generation: Cell<u64>,
    pub previewer: Option<Arc<LinkPreviewer>>,
}

impl Controller {
    pub fn new(app: &adw::Application, paths: Paths) -> Rc<Self> {
        let previewer = LinkPreviewer::new()
            .inspect_err(|e| tracing::warn!("link previews unavailable: {e}"))
            .ok();
        let this = Rc::new(Controller {
            app: app.clone(),
            state: RefCell::new(AppState::new(Some(paths))),
            client: RefCell::new(None),
            window: OnceCell::new(),
            update_queued: Cell::new(false),
            generation: Cell::new(0),
            previewer,
        });
        let window = Window::new(app, &this);
        let _ = this.window.set(window);
        this
    }

    pub fn window(&self) -> &Window {
        self.window.get().expect("window is set in new")
    }

    pub fn weak(self: &Rc<Self>) -> Weak<Self> {
        Rc::downgrade(self)
    }

    /// Changes the state, then redraws. Safe to call from any signal
    /// handler: if the state is busy (a redraw is running), it runs next.
    pub fn act(self: &Rc<Self>, f: impl FnOnce(&mut AppState) + 'static) {
        match self.state.try_borrow_mut() {
            Ok(mut state) => {
                f(&mut state);
                drop(state);
                self.queue_update();
            }
            Err(_) => {
                let this = self.clone();
                glib::timeout_add_local_once(Duration::ZERO, move || this.act(f));
            }
        }
    }

    /// Reads the state; `None` while it's being changed.
    pub fn read<T>(&self, f: impl FnOnce(&AppState) -> T) -> Option<T> {
        self.state.try_borrow().ok().map(|s| f(&s))
    }

    fn queue_update(self: &Rc<Self>) {
        if self.update_queued.replace(true) {
            return;
        }
        let this = self.clone();
        // Default priority: before GTK lays out and draws the next frame.
        glib::timeout_add_local_once(Duration::ZERO, move || {
            this.update_queued.set(false);
            this.update();
        });
    }

    fn update(self: &Rc<Self>) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            self.queue_update();
            return;
        };
        let mut changes = state.take_changes();
        drop(state);
        if changes.is_empty() {
            return;
        }
        let effects = std::mem::take(&mut changes.effects);
        tracing::debug!(
            phase = ?self.state.borrow().phase,
            structure = changes.structure,
            layout = changes.layout,
            messages = changes.messages.len(),
            "redraw"
        );
        {
            let state = self.state.borrow();
            self.window().refresh(self, &state, &changes);
        }
        for effect in effects {
            match effect {
                Effect::Notify { buffer, message } => {
                    let state = self.state.borrow();
                    crate::desktop::notify::post(&self.app, &state, &buffer, &message);
                }
                Effect::OpenDialog(dialog) => self.window().open_dialog(self, dialog),
                Effect::ScrollTo { buffer, message } => {
                    self.window().scroll_to(&buffer, &message);
                }
            }
        }
        if changes.buffers.len() + changes.messages.len() > 0 || changes.structure {
            let state = self.state.borrow();
            crate::desktop::notify::badge(&self.app, state.total_mentions());
        }
    }

    // MARK: Lifecycle

    fn client(&self) -> Option<Arc<BackroomClient>> {
        self.client.borrow().clone()
    }

    fn make_client(state: &AppState) -> Result<Arc<BackroomClient>, BackroomError> {
        let settings = state.settings();
        let paths = state
            .paths()
            .cloned()
            .unwrap_or_else(|| Paths::under(&std::env::temp_dir()));
        let config = settings.client_config(
            &paths.standalone_database(),
            state.text_log_path(),
            &client_version(),
        );
        BackroomClient::new(config, KeyringStore::new(settings.secret_account()))
    }

    /// Creates the client for the selected server and, with
    /// `auto_connect`, connects if a login is saved.
    pub fn start(self: &Rc<Self>, auto_connect: bool) {
        self.stop_session();
        let (settings, made) = {
            let state = self.state.borrow();
            let settings = state.settings();
            let made = settings.is_complete().then(|| Self::make_client(&state));
            (settings, made)
        };
        let client = match made {
            None => {
                self.act(|s| s.set_phase(Phase::LoggedOut));
                return;
            }
            Some(Err(e)) => {
                self.act(move |s| s.set_phase(Phase::Failed(describe(&e))));
                return;
            }
            Some(Ok(client)) => client,
        };
        *self.client.borrow_mut() = Some(client.clone());
        let dyn_client: Arc<dyn backroom_app::app::Client> = client.clone();
        self.act(move |s| {
            s.set_client(Some(dyn_client));
            s.set_phase(Phase::LoggedOut);
        });
        if settings.is_standalone() {
            // "No bouncer" has no login: it reconnects at launch if it was
            // connected when the app quit.
            let remembered = self.state.borrow().standalone_connected;
            tracing::info!(auto_connect, remembered, "no bouncer");
            if auto_connect && remembered {
                self.connect();
            }
            return;
        }
        if !auto_connect {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            // Reading the keyring can block (and prompt): not on this thread.
            let probe = client.clone();
            let has = gio::spawn_blocking(move || probe.has_login()).await;
            if !this.is_current(&client) {
                return;
            }
            match has {
                Ok(Ok(true)) => this.connect(),
                Ok(Ok(false)) => {}
                Ok(Err(e)) => this.act(move |s| s.set_phase(Phase::Failed(describe(&e)))),
                Err(_) => {
                    this.act(|s| s.set_phase(Phase::Failed("the keyring check crashed".into())))
                }
            }
        });
    }

    fn is_current(&self, client: &Arc<BackroomClient>) -> bool {
        self.client
            .borrow()
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, client))
    }

    /// Connects with the saved login and reads events until the session
    /// stops or another one starts.
    pub fn connect(self: &Rc<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.act(|s| s.begin_connect());
        tracing::info!("connecting");
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = client.connect().await {
                tracing::info!("couldn't connect: {e}");
                if this.generation.get() == generation {
                    this.act(move |s| s.connect_failed(&e));
                }
                return;
            }
            while let Some(event) = client.next_event().await {
                if this.generation.get() != generation {
                    break;
                }
                this.act(move |s| s.apply(event));
            }
            tracing::info!("session ended");
        });
    }

    fn stop_session(&self) {
        self.generation.set(self.generation.get() + 1);
        if let Some(client) = self.client.borrow_mut().take() {
            client.disconnect();
        }
        if let Ok(mut s) = self.state.try_borrow_mut() {
            s.set_client(None);
        }
    }

    /// Switches to another saved server and connects if it has a login.
    pub fn select_server(self: &Rc<Self>, id: Uuid) {
        {
            let mut state = self.state.borrow_mut();
            if state.servers.current().map(|s| s.id) == Some(id)
                || !state.servers.servers.iter().any(|s| s.id == id)
            {
                return;
            }
            state.servers.selected = Some(id);
            state.save_servers();
        }
        self.start(true);
    }

    /// Adds a server or saves changes to one. Changing the server in use
    /// starts over with it.
    pub fn save_server(self: &Rc<Self>, server: ServerSettings) {
        let restart = {
            let mut state = self.state.borrow_mut();
            let before = state.servers.current().cloned();
            state.servers.save(server);
            state.save_servers();
            !state
                .servers
                .current()
                .is_some_and(|c| c.connects_like(before.as_ref()))
        };
        self.queue_update();
        if restart {
            self.start(true);
        }
    }

    /// Forgets a server and its saved login.
    pub fn remove_server(self: &Rc<Self>, id: Uuid) {
        let (server, in_use) = {
            let state = self.state.borrow();
            let Some(server) = state.servers.servers.iter().find(|s| s.id == id).cloned() else {
                return;
            };
            (server, state.servers.current().map(|s| s.id) == Some(id))
        };
        if in_use {
            self.stop_session();
        }
        if server.is_complete() && !server.is_standalone() {
            // The login goes from the keyring.
            let account = server.secret_account();
            gio::spawn_blocking(move || {
                use backroom_ffi::SecretStore;
                if let Err(e) = KeyringStore::new(account).clear() {
                    tracing::warn!("couldn't delete a saved login: {e}");
                }
            });
        }
        {
            let mut state = self.state.borrow_mut();
            state.servers.remove(id);
            state.save_servers();
        }
        self.queue_update();
        if in_use {
            self.start(true);
        }
    }

    /// Saves the password of a password account and connects; soju checks
    /// it, and a wrong one comes back as a rejected login.
    pub fn log_in_with_password(self: &Rc<Self>, password: String) {
        let Some(client) = self.client() else {
            return;
        };
        if password.is_empty() || self.read(|s| s.phase == Phase::LoggingIn) != Some(false) {
            return;
        }
        self.act(|s| s.set_phase(Phase::LoggingIn));
        let this = self.clone();
        glib::spawn_future_local(async move {
            let saving = client.clone();
            let saved = gio::spawn_blocking(move || saving.login_with_password(password)).await;
            match saved {
                Ok(Ok(())) => {
                    this.act(|s| s.set_notice(None));
                    this.connect();
                }
                Ok(Err(e)) => this.act(move |s| {
                    s.set_phase(Phase::LoggedOut);
                    s.set_notice(Some(format!("Login failed: {}", describe(&e))));
                }),
                Err(_) => this.act(|s| s.set_phase(Phase::LoggedOut)),
            }
        });
    }

    /// Single sign-on: opens the login page in the browser; the app gets
    /// the `backroom://` callback through its desktop file (`handle_uri`).
    pub fn log_in_with_browser(self: &Rc<Self>) {
        let Some(client) = self.client() else {
            return;
        };
        if self.read(|s| s.phase == Phase::LoggingIn) != Some(false) {
            return;
        }
        self.act(|s| {
            s.set_phase(Phase::LoggingIn);
            s.set_notice(None);
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            match client.begin_login().await {
                Ok(url) => {
                    let launcher = gtk::UriLauncher::new(&url);
                    let window = this.window().widget();
                    let weak = this.weak();
                    launcher.launch(Some(&window), gio::Cancellable::NONE, move |result| {
                        if let (Err(e), Some(this)) = (result, weak.upgrade()) {
                            this.act(move |s| {
                                s.set_phase(Phase::LoggedOut);
                                s.set_notice(Some(format!("Couldn't open the browser: {e}")));
                            });
                        }
                    });
                }
                Err(e) => this.act(move |s| {
                    s.set_phase(Phase::LoggedOut);
                    s.set_notice(Some(format!("Login failed: {}", describe(&e))));
                }),
            }
        });
    }

    pub fn cancel_login(self: &Rc<Self>) {
        self.act(|s| {
            if s.phase == Phase::LoggingIn {
                s.set_phase(Phase::LoggedOut);
            }
        });
    }

    /// A URI the app was opened with: the login callback.
    pub fn handle_uri(self: &Rc<Self>, uri: String) {
        if !uri.starts_with(&format!("{CALLBACK_SCHEME}://")) {
            tracing::debug!("ignoring an unknown URI");
            return;
        }
        let Some(client) = self.client() else {
            return;
        };
        if self.read(|s| s.phase == Phase::LoggingIn) != Some(true) {
            self.act(|s| s.set_notice(Some("That login link has expired. Log in again.".into())));
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            match client.complete_login(uri).await {
                Ok(()) => this.connect(),
                Err(e) => this.act(move |s| {
                    s.set_phase(Phase::LoggedOut);
                    s.set_notice(Some(format!("Login failed: {}", describe(&e))));
                }),
            }
        });
    }

    /// Without a bouncer: connects to the saved networks.
    pub fn connect_standalone(self: &Rc<Self>) {
        if self.read(|s| s.is_standalone() && s.phase != Phase::Online) != Some(true) {
            return;
        }
        if self.client().is_none() {
            self.start(false);
        }
        self.act(|s| {
            s.set_notice(None);
            s.set_standalone_connected(true);
        });
        self.connect();
    }

    pub fn log_out(self: &Rc<Self>) {
        self.generation.set(self.generation.get() + 1);
        let client = self.client();
        self.act(move |s| {
            if s.is_standalone() {
                s.set_standalone_connected(false);
            }
            s.logged_out();
        });
        if let Some(client) = client {
            let this = self.clone();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || client.logout()).await;
                if let Ok(Err(e)) = result {
                    this.act(move |s| s.set_notice(Some(describe(&e))));
                }
            });
        }
    }

    /// Shows a buffer from a notification.
    pub fn open_buffer(self: &Rc<Self>, network: String, name: String) {
        self.window().present();
        self.act(move |s| s.reveal(&BufferId::new(network, &name)));
    }

    pub fn shutdown(&self) {
        self.stop_session();
    }
}

/// Our CTCP VERSION reply: "Backroom 0.1.0 (Linux; Ubuntu 26.04)".
pub fn client_version() -> String {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_owned())
        });
    let flatpak = std::path::Path::new("/.flatpak-info").exists();
    let mut detail = vec!["Linux".to_owned()];
    if flatpak {
        detail.push("Flatpak".into());
    } else if let Some(os) = os {
        detail.push(os);
    }
    format!(
        "Backroom {} ({})",
        env!("CARGO_PKG_VERSION"),
        detail.join("; ")
    )
}
