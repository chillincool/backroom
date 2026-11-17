//! The main window: sidebar | tabs of panes | member list, under one
//! header bar; the login page when logged out. Plus the app's actions and
//! keyboard shortcuts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use backroom_app::app::{AppState, Changes, Dialog, Phase};
use backroom_app::buffer::BufferId;
use backroom_app::panes::{PaneDirection, SplitAxis};
use backroom_app::storage;
use backroom_ffi::BufferKind;
use gtk::{gio, glib};
use serde::{Deserialize, Serialize};

use crate::desktop::controller::Controller;
use crate::desktop::dialogs::{self, Live};
use crate::desktop::login::LoginPage;
use crate::desktop::members::MemberList;
use crate::desktop::panes::Tabs;
use crate::desktop::sidebar::Sidebar;
use crate::desktop::style::{self, Palette, Probe};

/// Window size and panel widths, kept across launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Geometry {
    width: i32,
    height: i32,
    maximized: bool,
    sidebar: i32,
    members: i32,
    show_sidebar: bool,
    show_members: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            width: 1100,
            height: 720,
            maximized: false,
            sidebar: 220,
            members: 180,
            show_sidebar: true,
            show_members: true,
        }
    }
}

struct Inner {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    login: Rc<LoginPage>,
    title: adw::WindowTitle,
    sidebar: Rc<Sidebar>,
    outer: gtk::Paned,
    inner: gtk::Paned,
    members: Rc<MemberList>,
    sidebar_button: gtk::ToggleButton,
    members_button: gtk::ToggleButton,
    tabs: Rc<Tabs>,
    css: gtk::CssProvider,
    /// Styled like the window, for reading the desktop's accent colour.
    accent_probe: gtk::Label,
    palette: RefCell<Palette>,
    live: Rc<Live>,
    geometry: RefCell<Geometry>,
    geometry_path: Option<std::path::PathBuf>,
    started: Cell<bool>,
}

#[derive(Clone)]
pub struct Window(Rc<Inner>);

impl Window {
    pub fn new(app: &adw::Application, ctrl: &Rc<Controller>) -> Self {
        let (geometry_path, dark_pref) = {
            let state = ctrl.state.borrow();
            (
                state.paths().map(|p| p.state.join("window.json")),
                state.prefs.clone(),
            )
        };
        let geometry: Geometry = geometry_path
            .as_deref()
            .and_then(storage::read_json)
            .unwrap_or_default();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Backroom")
            .default_width(geometry.width)
            .default_height(geometry.height)
            .maximized(geometry.maximized)
            .width_request(360)
            .height_request(300)
            .build();

        // Header: sidebar toggle, new-chat menu, title, members, main menu.
        let header = adw::HeaderBar::new();
        let accent_probe = gtk::Label::builder()
            .css_classes(["accent"])
            .visible(false)
            .build();
        header.pack_start(&accent_probe);
        let title = adw::WindowTitle::new("Backroom", "");
        header.set_title_widget(Some(&title));
        let sidebar_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Show Sidebar (F9)")
            .active(geometry.show_sidebar)
            .build();
        header.pack_start(&sidebar_button);
        let new_menu = gio::Menu::new();
        new_menu.append(Some("Join Channel…"), Some("app.join-current"));
        new_menu.append(Some("Browse Channels…"), Some("app.browse-current"));
        new_menu.append(
            Some("New Conversation…"),
            Some("app.new-conversation-current"),
        );
        let net = gio::Menu::new();
        net.append(Some("Add Network…"), Some("app.add-network"));
        new_menu.append_section(None, &net);
        header.pack_start(
            &gtk::MenuButton::builder()
                .icon_name("list-add-symbolic")
                .menu_model(&new_menu)
                .tooltip_text("New")
                .build(),
        );
        let main_menu = gio::Menu::new();
        let view = gio::Menu::new();
        view.append(Some("New Tab"), Some("app.new-tab"));
        view.append(Some("Split Right"), Some("app.split-right"));
        view.append(Some("Split Down"), Some("app.split-down"));
        main_menu.append_section(None, &view);
        let find = gio::Menu::new();
        find.append(Some("Search…"), Some("app.search-current"));
        main_menu.append_section(None, &find);
        let app_section = gio::Menu::new();
        app_section.append(Some("Preferences"), Some("app.preferences"));
        app_section.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        app_section.append(Some("About Backroom"), Some("app.about"));
        main_menu.append_section(None, &app_section);
        let session = gio::Menu::new();
        session.append(Some("Log Out"), Some("app.log-out"));
        session.append(Some("Quit"), Some("app.quit"));
        main_menu.append_section(None, &session);
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&main_menu)
                .tooltip_text("Main Menu")
                .primary(true)
                .build(),
        );
        let members_button = gtk::ToggleButton::builder()
            .icon_name("system-users-symbolic")
            .tooltip_text("Show Members")
            .active(geometry.show_members)
            .build();
        header.pack_end(&members_button);

        let sidebar = Sidebar::new(ctrl);
        let tabs = Tabs::new(ctrl);
        let members = MemberList::new(ctrl);
        let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center.append(&tabs.bar);
        tabs.view.set_vexpand(true);
        center.append(&tabs.view);
        let inner = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&center)
            .end_child(&members.widget)
            .resize_start_child(true)
            .resize_end_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();
        let outer = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&sidebar.widget)
            .end_child(&inner)
            .resize_start_child(false)
            .resize_end_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .position(geometry.sidebar)
            .build();
        sidebar.widget.set_visible(geometry.show_sidebar);
        let chat = adw::ToolbarView::new();
        chat.add_top_bar(&header);
        chat.set_content(Some(&outer));

        let login = LoginPage::new(ctrl);
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&login.widget, Some("login"));
        stack.add_named(&chat, Some("chat"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));

        let css = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let dark = adw::StyleManager::default().is_dark();
        let theme = dark_pref.theme(dark);
        let palette = Palette::new(&theme, dark, &Probe::read(dark, &accent_probe));

        let w = Window(Rc::new(Inner {
            window,
            toasts,
            stack,
            login,
            title,
            sidebar,
            outer,
            inner,
            members,
            sidebar_button,
            members_button,
            tabs,
            css,
            accent_probe,
            palette: RefCell::new(palette),
            live: Rc::default(),
            geometry: RefCell::new(geometry),
            geometry_path,
            started: Cell::new(false),
        }));
        w.connect(ctrl);
        w
    }

    pub fn widget(&self) -> adw::ApplicationWindow {
        self.0.window.clone()
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    fn connect(&self, ctrl: &Rc<Controller>) {
        let i = &self.0;
        let sidebar = i.sidebar.widget.clone();
        i.sidebar_button
            .connect_toggled(move |b| sidebar.set_visible(b.is_active()));
        let c = ctrl.weak();
        i.members_button.connect_toggled(move |_| {
            if let Some(c) = c.upgrade() {
                c.window().clone().update_members_panel(&c);
            }
        });
        // Buffers are only marked read while the window is active.
        let c = ctrl.weak();
        i.window.connect_is_active_notify(move |w| {
            let active = w.is_active();
            if let Some(c) = c.upgrade() {
                c.act(move |s| s.set_active(active));
            }
        });
        // Closing the window quits, unless running in the background.
        let c = ctrl.weak();
        i.window.connect_close_request(move |w| {
            let Some(c) = c.upgrade() else {
                return glib::Propagation::Proceed;
            };
            c.window().save_geometry();
            let background = c.read(|s| s.prefs.run_in_background).unwrap_or(false);
            if background {
                w.set_visible(false);
                c.act(|s| s.set_active(false));
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        // Light and dark: the theme for each, and the desktop's colours.
        let c = ctrl.weak();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(c) = c.upgrade() {
                let w = c.window().clone();
                c.read(|s| w.restyle(s));
            }
        });
        let c = ctrl.weak();
        adw::StyleManager::default().connect_notify_local(Some("accent-color"), move |_, _| {
            if let Some(c) = c.upgrade() {
                let w = c.window().clone();
                c.read(|s| w.restyle(s));
            }
        });
    }

    pub fn save_geometry(&self) {
        let i = &self.0;
        let mut g = i.geometry.borrow_mut();
        g.maximized = i.window.is_maximized();
        if !g.maximized {
            let (w, h) = i.window.default_size();
            g.width = w;
            g.height = h;
        }
        g.show_sidebar = i.sidebar_button.is_active();
        g.show_members = i.members_button.is_active();
        if i.sidebar.widget.is_visible() {
            g.sidebar = i.outer.position();
        }
        if i.members.widget.is_visible() && i.inner.width() > 0 {
            g.members = i.inner.width() - i.inner.position();
        }
        if let Some(path) = &i.geometry_path {
            storage::write_json(path, &*g);
        }
    }

    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(text));
        toast.set_timeout(5);
        self.0.toasts.add_toast(toast);
    }

    fn restyle(&self, state: &AppState) {
        let dark = adw::StyleManager::default().is_dark();
        let theme = state.prefs.theme(dark);
        self.0
            .css
            .load_from_string(&style::css(&theme, &state.prefs));
        let palette = Palette::new(&theme, dark, &Probe::read(dark, &self.0.accent_probe));
        *self.0.palette.borrow_mut() = palette.clone();
        self.0.tabs.restyle(state, &palette);
        self.0.members.refresh(state, &palette, true);
    }

    fn update_members_panel(&self, ctrl: &Rc<Controller>) {
        let i = &self.0;
        let shown = ctrl
            .read(|s| {
                let palette = i.palette.borrow().clone();
                i.members.refresh(s, &palette, false)
            })
            .unwrap_or(false);
        let visible = shown && i.members_button.is_active();
        let was = i.members.widget.is_visible();
        i.members.widget.set_visible(visible);
        i.members_button.set_sensitive(shown);
        if visible && !was {
            // Back to the width it had.
            let width = i.geometry.borrow().members;
            let inner = i.inner.clone();
            glib::idle_add_local_once(move || {
                if inner.width() > width {
                    inner.set_position(inner.width() - width);
                }
            });
        } else if was && !visible && i.inner.width() > 0 {
            i.geometry.borrow_mut().members = i.inner.width() - i.inner.position();
        }
    }

    /// Redraws what changed.
    pub fn refresh(&self, ctrl: &Rc<Controller>, state: &AppState, changes: &Changes) {
        let i = &self.0;
        let first = !i.started.replace(true);
        if first || changes.preferences {
            crate::desktop::settings::apply_appearance(state);
            self.restyle(state);
        }
        let online = state.phase == Phase::Online;
        if first || changes.phase {
            i.stack
                .set_visible_child_name(if online { "chat" } else { "login" });
        }
        if first || changes.phase || changes.servers || changes.notice || changes.preferences {
            i.login.refresh(state);
        }
        if online
            && changes.notice
            && let Some(notice) = &state.notice
        {
            self.toast(notice);
            ctrl.act(|s| s.notice = None);
        }
        if !online {
            i.title.set_title("Backroom");
            i.title.set_subtitle("");
            i.window.set_title(Some("Backroom"));
            return;
        }
        i.sidebar.refresh(ctrl, state, changes);
        let palette = i.palette.borrow().clone();
        i.tabs.refresh(ctrl, state, changes, &palette);
        if first
            || changes.layout
            || changes.structure
            || changes.preferences
            || state
                .selection()
                .is_some_and(|s| changes.buffers.contains(&s))
        {
            let shown = i.members.refresh(state, &palette, false);
            let visible = shown && i.members_button.is_active();
            if visible != i.members.widget.is_visible() {
                self.update_members_panel(ctrl);
            }
            i.members_button.set_sensitive(shown);
        }
        // Title: the focused conversation, its network below.
        let (title, subtitle) = match state.selected_buffer() {
            Some(b) => (
                state.buffer_title(&b.id),
                if b.kind == BufferKind::Server {
                    String::new()
                } else {
                    state.network_name(&b.id.network)
                },
            ),
            None => ("Backroom".to_owned(), String::new()),
        };
        i.title.set_title(&title);
        i.title.set_subtitle(&subtitle);
        i.window.set_title(Some(&if title == "Backroom" {
            title.clone()
        } else {
            format!("{title} – Backroom")
        }));
        i.live.refresh(state, changes);
        // Read conversations' notifications go.
        for id in &changes.buffers {
            if state.buffers.get(id).is_some_and(|b| b.unread() == 0) {
                crate::desktop::notify::withdraw(&ctrl.app, id);
            }
        }
    }

    pub fn open_dialog(&self, ctrl: &Rc<Controller>, dialog: Dialog) {
        let parent: gtk::Widget = self.0.window.clone().upcast();
        match dialog {
            Dialog::JoinChannel { network } => dialogs::join_channel(ctrl, &parent, network),
            Dialog::BrowseChannels { network } => {
                dialogs::browse_channels(ctrl, &self.0.live, &parent, network);
            }
            Dialog::NewConversation { network } => {
                dialogs::new_conversation(ctrl, &parent, network);
            }
            Dialog::Whois { network, info } => dialogs::whois(ctrl, &parent, network, *info),
            Dialog::Search { network } => {
                let buffer = ctrl
                    .read(|s| {
                        s.selected_buffer()
                            .filter(|b| b.id.network == network && b.kind != BufferKind::Server)
                            .map(|b| b.name.clone())
                    })
                    .flatten();
                dialogs::search(ctrl, &self.0.live, &parent, network, buffer);
            }
            Dialog::ChannelSettings { network, channel } => {
                // Opened from the state (it asks for the settings); the
                // dialog asks again, which is harmless.
                let open = self
                    .0
                    .window
                    .visible_dialog()
                    .is_some_and(|d| d.title() == channel);
                if !open {
                    dialogs::channel_settings(ctrl, &self.0.live, &parent, network, channel);
                }
            }
        }
    }

    pub fn scroll_to(&self, buffer: &BufferId, message: &str) {
        if let Some(pane) = self.0.tabs.pane_showing(buffer) {
            pane.scroll_to(message);
        }
    }

    pub fn focus_composer(&self, state: &AppState) {
        if let Some(p) = self.0.tabs.focused_pane(state) {
            p.focus_composer();
        }
    }
}

// MARK: Actions

type Action = Rc<dyn Fn(&Rc<Controller>)>;
type NetworkAction = Rc<dyn Fn(&Rc<Controller>, String)>;
type BufferAction = Rc<dyn Fn(&Rc<Controller>, BufferId, String)>;

fn parent(c: &Rc<Controller>) -> gtk::Widget {
    c.window().widget().upcast()
}

pub fn install_actions(app: &adw::Application, ctrl: &Rc<Controller>) {
    let add = |name: &str, f: Action| {
        let action = gio::SimpleAction::new(name, None);
        let c = ctrl.weak();
        action.connect_activate(move |_, _| {
            if let Some(c) = c.upgrade() {
                f(&c);
            }
        });
        app.add_action(&action);
    };
    let add_s = |name: &str, f: NetworkAction| {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let c = ctrl.weak();
        action.connect_activate(move |_, v| {
            if let (Some(c), Some(s)) = (c.upgrade(), v.and_then(|v| v.get::<String>())) {
                f(&c, s);
            }
        });
        app.add_action(&action);
    };
    let add_ss = |name: &str, f: BufferAction| {
        let ty = glib::VariantTy::new("(ss)").expect("a valid type");
        let action = gio::SimpleAction::new(name, Some(ty));
        let c = ctrl.weak();
        action.connect_activate(move |_, v| {
            if let (Some(c), Some((network, name))) =
                (c.upgrade(), v.and_then(|v| v.get::<(String, String)>()))
            {
                f(&c, BufferId::new(network.clone(), &name), name);
            }
        });
        app.add_action(&action);
    };
    let current = |c: &Rc<Controller>| c.read(|s| s.current_network()).flatten();
    let on_current = move |dialog: fn(String) -> Dialog| -> Action {
        Rc::new(move |c: &Rc<Controller>| match current(c) {
            Some(n) => c.act(move |s| s.open(dialog(n))),
            None => c.window().toast("Add a network first."),
        })
    };

    // App
    add(
        "quit",
        Rc::new(|c| {
            c.window().save_geometry();
            c.app.quit();
        }),
    );
    add(
        "preferences",
        Rc::new(|c| crate::desktop::settings::show(c, &parent(c), None)),
    );
    add("about", Rc::new(|c| about(&parent(c))));
    add("shortcuts", Rc::new(|c| shortcuts(&c.window().widget())));
    add("log-out", Rc::new(|c| c.log_out()));
    add(
        "add-server",
        Rc::new(|c| dialogs::server_editor(c, &parent(c), None)),
    );
    add(
        "edit-server",
        Rc::new(|c| {
            let server = c.read(|s| s.settings());
            dialogs::server_editor(c, &parent(c), server);
        }),
    );
    let open_buffer = gio::SimpleAction::new(
        "open-buffer",
        Some(glib::VariantTy::new("(ss)").expect("type")),
    );
    let c = ctrl.weak();
    open_buffer.connect_activate(move |_, v| {
        if let (Some(c), Some((network, name))) =
            (c.upgrade(), v.and_then(|v| v.get::<(String, String)>()))
        {
            c.open_buffer(network, name);
        }
    });
    app.add_action(&open_buffer);

    // Window
    add(
        "toggle-sidebar",
        Rc::new(|c| {
            let b = &c.window().0.sidebar_button;
            b.set_active(!b.is_active());
        }),
    );
    add(
        "toggle-members",
        Rc::new(|c| {
            let b = &c.window().0.members_button;
            b.set_active(!b.is_active());
        }),
    );
    add("new-tab", Rc::new(|c| c.act(|s| s.new_tab())));
    add("close-pane", Rc::new(|c| c.act(|s| s.close_pane(None))));
    add(
        "split-right",
        Rc::new(|c| c.act(|s| s.split(SplitAxis::Horizontal, None))),
    );
    add(
        "split-down",
        Rc::new(|c| c.act(|s| s.split(SplitAxis::Vertical, None))),
    );
    add("next-tab", Rc::new(|c| c.act(|s| s.cycle_tab(1))));
    add("prev-tab", Rc::new(|c| c.act(|s| s.cycle_tab(-1))));
    for (name, dir) in [
        ("focus-left", PaneDirection::Left),
        ("focus-right", PaneDirection::Right),
        ("focus-up", PaneDirection::Up),
        ("focus-down", PaneDirection::Down),
    ] {
        add(name, Rc::new(move |c| c.act(move |s| s.move_focus(dir))));
    }
    for n in 1..=9 {
        add(
            &format!("tab-{n}"),
            Rc::new(move |c| c.act(move |s| s.show_tab_number(n))),
        );
    }
    for (name, step) in [("prev-buffer", -1isize), ("next-buffer", 1)] {
        add(name, Rc::new(move |c| c.act(move |s| step_buffer(s, step))));
    }
    add(
        "focus-composer",
        Rc::new(|c| {
            let w = c.window().clone();
            c.read(|s| w.focus_composer(s));
        }),
    );

    // Chat, on the current network
    add(
        "join-current",
        on_current(|network| Dialog::JoinChannel { network }),
    );
    add(
        "browse-current",
        on_current(|network| Dialog::BrowseChannels { network }),
    );
    add(
        "new-conversation-current",
        on_current(|network| Dialog::NewConversation { network }),
    );
    add(
        "search-current",
        on_current(|network| Dialog::Search { network }),
    );
    add(
        "channel-settings-current",
        Rc::new(|c| {
            let target = c
                .read(|s| {
                    s.selected_buffer()
                        .filter(|b| b.kind == BufferKind::Channel)
                        .map(|b| (b.id.network.clone(), b.name.clone()))
                })
                .flatten();
            if let Some((network, channel)) = target {
                dialogs::channel_settings(c, &c.window().0.live, &parent(c), network, channel);
            }
        }),
    );
    add(
        "close-buffer-current",
        Rc::new(|c| {
            c.act(|s| {
                if let Some(id) = s.selection() {
                    if s.buffers
                        .get(&id)
                        .is_some_and(|b| b.kind == BufferKind::Server)
                    {
                        s.set_notice(Some("The server buffer can't be closed.".into()));
                    } else {
                        s.close(&id);
                    }
                }
            });
        }),
    );
    add(
        "add-network",
        Rc::new(|c| dialogs::network_form(c, &parent(c), None)),
    );

    // Chat, on a given network (sidebar menus)
    add_s(
        "join",
        Rc::new(|c, n| c.act(move |s| s.open(Dialog::JoinChannel { network: n }))),
    );
    add_s(
        "browse",
        Rc::new(|c, n| c.act(move |s| s.open(Dialog::BrowseChannels { network: n }))),
    );
    add_s(
        "new-conversation",
        Rc::new(|c, n| {
            c.act(move |s| s.open(Dialog::NewConversation { network: n }));
        }),
    );
    add_s(
        "search",
        Rc::new(|c, n| c.act(move |s| s.open(Dialog::Search { network: n }))),
    );
    add_s("away", Rc::new(|c, n| c.act(move |s| s.set_away(&n, true))));
    add_s(
        "back",
        Rc::new(|c, n| c.act(move |s| s.set_away(&n, false))),
    );
    add_s(
        "edit-network",
        Rc::new(|c, n| dialogs::network_form(c, &parent(c), Some(n))),
    );
    add_s(
        "remove-network",
        Rc::new(|c, n| dialogs::remove_network(c, &parent(c), n)),
    );

    // A given buffer (sidebar menus)
    add_ss(
        "buffer-new-tab",
        Rc::new(|c, id, _| {
            c.act(move |s| {
                s.new_tab();
                s.select(Some(id));
            });
        }),
    );
    add_ss(
        "buffer-split-right",
        Rc::new(|c, id, _| {
            c.act(move |s| s.split(SplitAxis::Horizontal, Some(id)));
        }),
    );
    add_ss(
        "buffer-split-down",
        Rc::new(|c, id, _| {
            c.act(move |s| s.split(SplitAxis::Vertical, Some(id)));
        }),
    );
    add_ss(
        "close-buffer",
        Rc::new(|c, id, _| c.act(move |s| s.close(&id))),
    );
    add_ss(
        "channel-settings",
        Rc::new(|c, id, name| {
            dialogs::channel_settings(c, &c.window().0.live, &parent(c), id.network, name);
        }),
    );

    // Debug builds: `gapplication action net.chillincool.backroom
    // screenshot "'/path/shot.png'"` renders the window to a PNG, drawn or
    // not (for checking layouts without a visible window).
    #[cfg(debug_assertions)]
    add_s(
        "screenshot",
        Rc::new(|c, path| screenshot(&c.window().widget(), &path)),
    );

    for (action, accels) in ACCELS {
        app.set_accels_for_action(action, accels);
    }
}

const ACCELS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Ctrl>q"]),
    ("app.preferences", &["<Ctrl>comma"]),
    ("app.shortcuts", &["<Ctrl>question"]),
    ("app.toggle-sidebar", &["F9"]),
    ("app.toggle-members", &["<Ctrl><Shift>m"]),
    ("app.new-tab", &["<Ctrl>t"]),
    ("app.close-pane", &["<Ctrl>w"]),
    ("app.split-right", &["<Ctrl>d"]),
    ("app.split-down", &["<Ctrl><Shift>d"]),
    ("app.next-tab", &["<Ctrl>Page_Down", "<Ctrl>Tab"]),
    ("app.prev-tab", &["<Ctrl>Page_Up", "<Ctrl><Shift>Tab"]),
    ("app.focus-left", &["<Ctrl><Alt>Left"]),
    ("app.focus-right", &["<Ctrl><Alt>Right"]),
    ("app.focus-up", &["<Ctrl><Alt>Up"]),
    ("app.focus-down", &["<Ctrl><Alt>Down"]),
    ("app.prev-buffer", &["<Alt>Up"]),
    ("app.next-buffer", &["<Alt>Down"]),
    ("app.join-current", &["<Ctrl>j"]),
    ("app.new-conversation-current", &["<Ctrl>n"]),
    ("app.search-current", &["<Ctrl>f"]),
    ("app.focus-composer", &["<Ctrl>l"]),
    ("app.tab-1", &["<Alt>1"]),
    ("app.tab-2", &["<Alt>2"]),
    ("app.tab-3", &["<Alt>3"]),
    ("app.tab-4", &["<Alt>4"]),
    ("app.tab-5", &["<Alt>5"]),
    ("app.tab-6", &["<Alt>6"]),
    ("app.tab-7", &["<Alt>7"]),
    ("app.tab-8", &["<Alt>8"]),
    ("app.tab-9", &["<Alt>9"]),
];

#[cfg(debug_assertions)]
fn screenshot(window: &adw::ApplicationWindow, path: &str) {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) else {
        tracing::warn!("screenshot: nothing to render");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    if let Err(e) = texture.save_to_png(path) {
        tracing::warn!("screenshot: {e}");
    }
}

/// The buffer above or below the focused one in the sidebar's order.
fn step_buffer(s: &mut AppState, step: isize) {
    let order: Vec<BufferId> = s
        .networks
        .iter()
        .flat_map(|n| n.buffers.iter().cloned())
        .filter(|b| s.buffers.contains_key(b))
        .collect();
    if order.is_empty() {
        return;
    }
    let at = s
        .selection()
        .and_then(|sel| order.iter().position(|b| *b == sel));
    let n = order.len() as isize;
    let next = match at {
        Some(i) => ((i as isize + step) % n + n) % n,
        None => 0,
    };
    s.select(Some(order[next as usize].clone()));
}

fn about(parent: &gtk::Widget) {
    let about = adw::AboutDialog::builder()
        .application_name("Backroom")
        .application_icon(crate::desktop::APP_ID)
        .developer_name("chillincool")
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/chillincool/backroom")
        .issue_url("https://github.com/chillincool/backroom/issues")
        .license_type(gtk::License::MitX11)
        .comments("An IRC client for the soju bouncer, or for IRC networks directly.")
        .build();
    about.present(Some(parent));
}

fn shortcuts(window: &adw::ApplicationWindow) {
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Conversations",
            &[
                ("<Alt>Up", "Previous conversation"),
                ("<Alt>Down", "Next conversation"),
                ("<Ctrl>j", "Join a channel"),
                ("<Ctrl>n", "New conversation"),
                ("<Ctrl>f", "Search"),
                ("<Ctrl>l", "Go to the message box"),
                ("Tab", "Complete a nick, channel or command"),
                ("<Shift>Return", "New line in a message"),
            ],
        ),
        (
            "Tabs and panes",
            &[
                ("<Ctrl>t", "New tab"),
                ("<Ctrl>w", "Close pane (or tab)"),
                ("<Ctrl>d", "Split right"),
                ("<Ctrl><Shift>d", "Split down"),
                ("<Ctrl><Alt>Left", "Pane to the left (also Right, Up, Down)"),
                ("<Ctrl>Page_Down", "Next tab"),
                ("<Ctrl>Page_Up", "Previous tab"),
                ("<Alt>1", "Tab 1 (to 9: the last)"),
            ],
        ),
        (
            "Window",
            &[
                ("F9", "Show or hide the sidebar"),
                ("<Ctrl><Shift>m", "Show or hide members"),
                ("<Ctrl>comma", "Preferences"),
                ("<Ctrl>q", "Quit"),
            ],
        ),
    ];
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    for (title, rows) in groups {
        let group = adw::PreferencesGroup::builder().title(*title).build();
        for (accel, label) in *rows {
            let row = adw::ActionRow::builder().title(*label).build();
            let shortcut = gtk::ShortcutLabel::new(accel);
            shortcut.set_valign(gtk::Align::Center);
            row.add_suffix(&shortcut);
            group.add(&row);
        }
        content.append(&group);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(480)
        .content_height(640)
        .child(&view)
        .build();
    dialog.present(Some(window));
}
