//! The sidebar: networks and their buffers, with unread counts, and the
//! connection's state at the foot. A network's header row is also its
//! server buffer (server messages, MOTD, replies); its arrow folds it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use backroom_app::app::{AppState, Changes};
use backroom_app::buffer::{BufferId, connection_summary};
use backroom_ffi::{BufferKind, ConnectionState};
use gtk::{gio, glib};

use crate::desktop::controller::Controller;

struct BufferRow {
    row: gtk::ListBoxRow,
    name: gtk::Label,
    badge: gtk::Label,
    dot: gtk::Box,
}

struct NetworkRow {
    dot: gtk::Box,
    name: gtk::Label,
    away: gtk::Label,
}

pub struct Sidebar {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    status: gtk::Label,
    status_dot: gtk::Box,
    rows: RefCell<HashMap<BufferId, BufferRow>>,
    networks: RefCell<HashMap<String, NetworkRow>>,
    collapsed: RefCell<HashSet<String>>,
    syncing: Rc<Cell<bool>>,
}

impl Sidebar {
    pub fn new(ctrl: &Rc<Controller>) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let status = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build();
        let status_dot = dot();
        status_dot.set_valign(gtk::Align::Center);
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Network…")
            .action_name("app.add-network")
            .css_classes(["flat"])
            .build();
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.set_margin_start(12);
        footer.set_margin_end(6);
        footer.set_margin_top(4);
        footer.set_margin_bottom(4);
        footer.append(&status_dot);
        footer.append(&status);
        footer.append(&add);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(160);
        widget.append(&scroller);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&footer);

        let sidebar = Rc::new(Sidebar {
            widget,
            list,
            status,
            status_dot,
            rows: RefCell::default(),
            networks: RefCell::default(),
            collapsed: RefCell::default(),
            syncing: Rc::new(Cell::new(false)),
        });
        let (weak, c) = (Rc::downgrade(&sidebar), ctrl.weak());
        sidebar.list.connect_row_selected(move |_, row| {
            let (Some(s), Some(c), Some(row)) = (weak.upgrade(), c.upgrade(), row) else {
                return;
            };
            if s.syncing.get() {
                return;
            }
            let id = s
                .rows
                .borrow()
                .iter()
                .find(|(_, r)| &r.row == row)
                .map(|(id, _)| id.clone());
            if let Some(id) = id {
                c.act(move |st| st.select(Some(id)));
            }
        });
        sidebar
    }

    pub fn refresh(self: &Rc<Self>, ctrl: &Rc<Controller>, state: &AppState, changes: &Changes) {
        self.syncing.set(true);
        if changes.structure || self.rows.borrow().is_empty() && !state.networks.is_empty() {
            self.rebuild(ctrl, state);
        }
        if changes.connection || changes.structure {
            self.refresh_networks(state);
        }
        let all = changes.structure || changes.preferences || changes.layout;
        for (id, row) in self.rows.borrow().iter() {
            if all || changes.buffers.contains(id) || changes.messages.contains(id) {
                update_row(state, id, row);
            }
        }
        // The focused pane's buffer is selected.
        let selection = state.selection();
        let rows = self.rows.borrow();
        match selection.as_ref().and_then(|id| rows.get(id)) {
            Some(r) => {
                if self.list.selected_row().as_ref() != Some(&r.row) {
                    self.list.select_row(Some(&r.row));
                }
            }
            None => self.list.unselect_all(),
        }
        self.syncing.set(false);
    }

    fn refresh_networks(&self, state: &AppState) {
        let control = state.control.as_ref();
        let standalone = state.is_standalone();
        let (text, connected) = match control {
            _ if standalone => (
                format!(
                    "No bouncer · {} network{}",
                    state.networks.len(),
                    if state.networks.len() == 1 { "" } else { "s" }
                ),
                true,
            ),
            Some(c) => (
                connection_summary(c),
                matches!(c, ConnectionState::Connected),
            ),
            None => ("Offline".to_owned(), false),
        };
        self.status.set_text(&text);
        self.status.set_tooltip_text(Some(&text));
        set_dot(&self.status_dot, connected, control.is_none());
        for n in &state.networks {
            if let Some(row) = self.networks.borrow().get(n.id()) {
                row.name.set_text(&n.network.name);
                let connected = matches!(n.connection, ConnectionState::Connected);
                set_dot(&row.dot, connected, false);
                let mut tip = connection_summary(&n.connection);
                if let Some(error) = n.network.attributes.get("error").filter(|e| !e.is_empty()) {
                    tip = format!("{tip}\n{error}");
                }
                if let Some(nick) = &n.nick {
                    tip = format!("{tip}\nYou are {nick}");
                }
                row.dot.set_tooltip_text(Some(&tip));
                row.away.set_visible(n.away);
            }
        }
    }

    fn rebuild(self: &Rc<Self>, ctrl: &Rc<Controller>, state: &AppState) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.rows.borrow_mut().clear();
        self.networks.borrow_mut().clear();
        for n in &state.networks {
            let id = n.id().to_owned();
            let collapsed = self.collapsed.borrow().contains(&id);
            let server = n.buffers.iter().find(|b| {
                state
                    .buffers
                    .get(*b)
                    .is_some_and(|m| m.kind == BufferKind::Server)
            });
            self.list
                .append(&self.network_header(ctrl, state, &id, server, collapsed));
            if collapsed {
                continue;
            }
            for buffer in &n.buffers {
                let Some(model) = state.buffers.get(buffer) else {
                    continue;
                };
                if model.kind == BufferKind::Server {
                    continue;
                }
                let row = buffer_row(ctrl, state, buffer, model.kind);
                self.list.append(&row.row);
                self.rows.borrow_mut().insert(buffer.clone(), row);
            }
        }
        if state.networks.is_empty() {
            let hint = gtk::Label::builder()
                .label(if state.is_standalone() {
                    "No networks yet.\nAdd one with +."
                } else {
                    "No networks yet."
                })
                .justify(gtk::Justification::Center)
                .css_classes(["dim-label"])
                .margin_top(24)
                .build();
            let row = gtk::ListBoxRow::builder()
                .child(&hint)
                .selectable(false)
                .activatable(false)
                .build();
            self.list.append(&row);
        }
    }

    fn network_header(
        self: &Rc<Self>,
        ctrl: &Rc<Controller>,
        state: &AppState,
        id: &str,
        server: Option<&BufferId>,
        collapsed: bool,
    ) -> gtk::ListBoxRow {
        let dot = dot();
        dot.set_valign(gtk::Align::Center);
        let name = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .label(state.network_name(id))
            .build();
        let away = gtk::Label::builder()
            .label("away")
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        let badge = gtk::Label::builder()
            .css_classes(["sidebar-badge"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let arrow = gtk::Button::builder()
            .icon_name(if collapsed {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            })
            .tooltip_text(if collapsed {
                "Show Conversations"
            } else {
                "Hide Conversations"
            })
            .css_classes(["flat", "circular", "sidebar-fold", "dim-label"])
            .valign(gtk::Align::Center)
            .build();
        let server_name = server
            .and_then(|b| state.buffers.get(b))
            .map(|m| m.name.clone());
        let menu = network_menu(
            id,
            state.network(id).is_some_and(|n| n.away),
            server_name.as_deref(),
        );
        let button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .tooltip_text("Network")
            .build();
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        b.append(&arrow);
        b.append(&name);
        b.append(&away);
        b.append(&badge);
        b.append(&dot);
        b.append(&button);
        let row = gtk::ListBoxRow::builder()
            .child(&b)
            .selectable(server.is_some())
            .activatable(server.is_some())
            .css_classes(["network-row"])
            .build();
        // Right-click: the network's menu; middle-click: its server
        // messages in a new tab.
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&b);
        popover.set_has_arrow(false);
        let right = gtk::GestureClick::builder().button(3).build();
        let p = popover.clone();
        right.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            p.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            p.popup();
        });
        b.add_controller(right);
        b.connect_destroy(move |_| popover.unparent());
        if let Some(server) = server {
            b.add_controller(new_tab_on_middle_click(ctrl, server));
        }
        // The arrow folds the network.
        let (weak, c, id2) = (Rc::downgrade(self), ctrl.weak(), id.to_owned());
        arrow.connect_clicked(move |_| {
            let (Some(s), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                return;
            };
            {
                let mut collapsed = s.collapsed.borrow_mut();
                if !collapsed.remove(&id2) {
                    collapsed.insert(id2.clone());
                }
            }
            c.read(|st| {
                let changes = Changes {
                    structure: true,
                    ..Changes::default()
                };
                s.refresh(&c, st, &changes);
            });
        });
        if let Some(server) = server {
            let r = BufferRow {
                row: row.clone(),
                name: name.clone(),
                badge,
                // Server buffers have no presence; this one is never shown.
                dot: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            };
            update_row(state, server, &r);
            self.rows.borrow_mut().insert(server.clone(), r);
        }
        self.networks
            .borrow_mut()
            .insert(id.to_owned(), NetworkRow { dot, name, away });
        row
    }
}

fn dot() -> gtk::Box {
    let d = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    d.add_css_class("status-dot");
    d.set_halign(gtk::Align::Center);
    d
}

fn set_dot(dot: &gtk::Box, online: bool, offline: bool) {
    dot.remove_css_class("online");
    dot.remove_css_class("offline");
    if online {
        dot.add_css_class("online");
    } else if offline {
        dot.add_css_class("offline");
    }
}

/// `server`: the server buffer's name, for opening it elsewhere.
fn network_menu(network: &str, away: bool, server: Option<&str>) -> gio::Menu {
    let target = network.to_variant();
    let item = |label: &str, action: &str| {
        let i = gio::MenuItem::new(Some(label), None);
        i.set_action_and_target_value(Some(action), Some(&target));
        i
    };
    let menu = gio::Menu::new();
    if let Some(name) = server {
        menu.append_section(None, &open_menu(network, name));
    }
    let chat = gio::Menu::new();
    chat.append_item(&item("Join Channel…", "app.join"));
    chat.append_item(&item("Browse Channels…", "app.browse"));
    chat.append_item(&item("New Conversation…", "app.new-conversation"));
    chat.append_item(&item("Search…", "app.search"));
    menu.append_section(None, &chat);
    let status = gio::Menu::new();
    status.append_item(&if away {
        item("Set Back", "app.back")
    } else {
        item("Set Away", "app.away")
    });
    menu.append_section(None, &status);
    let edit = gio::Menu::new();
    edit.append_item(&item("Edit Network…", "app.edit-network"));
    edit.append_item(&item("Remove Network…", "app.remove-network"));
    menu.append_section(None, &edit);
    menu
}

fn buffer_menu(id: &BufferId, name: &str, kind: BufferKind) -> gio::Menu {
    let target = (id.network.clone(), name.to_owned()).to_variant();
    let item = |label: &str, action: &str| {
        let i = gio::MenuItem::new(Some(label), None);
        i.set_action_and_target_value(Some(action), Some(&target));
        i
    };
    let menu = gio::Menu::new();
    menu.append_section(None, &open_menu(&id.network, name));
    let more = gio::Menu::new();
    if kind == BufferKind::Channel {
        more.append_item(&item("Channel Settings…", "app.channel-settings"));
    }
    if kind != BufferKind::Server {
        more.append_item(&item(
            if kind == BufferKind::Channel {
                "Leave Channel"
            } else {
                "Close Conversation"
            },
            "app.close-buffer",
        ));
    }
    menu.append_section(None, &more);
    menu
}

/// Opening a buffer in a new tab or a split.
fn open_menu(network: &str, name: &str) -> gio::Menu {
    let target = (network.to_owned(), name.to_owned()).to_variant();
    let open = gio::Menu::new();
    for (label, action) in [
        ("Open in New Tab", "app.buffer-new-tab"),
        ("Open in Split Right", "app.buffer-split-right"),
        ("Open in Split Down", "app.buffer-split-down"),
    ] {
        let i = gio::MenuItem::new(Some(label), None);
        i.set_action_and_target_value(Some(action), Some(&target));
        open.append_item(&i);
    }
    open
}

fn new_tab_on_middle_click(ctrl: &Rc<Controller>, id: &BufferId) -> gtk::GestureClick {
    let middle = gtk::GestureClick::builder().button(2).build();
    let (c, target) = (ctrl.weak(), id.clone());
    middle.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(c) = c.upgrade() {
            let target = target.clone();
            c.act(move |s| {
                s.new_tab();
                s.select(Some(target));
            });
        }
    });
    middle
}

fn buffer_row(
    ctrl: &Rc<Controller>,
    state: &AppState,
    id: &BufferId,
    kind: BufferKind,
) -> BufferRow {
    let model = &state.buffers[id];
    let icon = gtk::Image::from_icon_name(match kind {
        BufferKind::Server => "network-server-symbolic",
        BufferKind::Channel => "chat-symbolic",
        BufferKind::Query => "avatar-default-symbolic",
    });
    icon.add_css_class("dim-label");
    let name = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let badge = gtk::Label::builder()
        .css_classes(["sidebar-badge"])
        .visible(false)
        .build();
    let presence = dot();
    presence.set_valign(gtk::Align::Center);
    presence.set_visible(false);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.set_margin_start(8);
    b.append(&icon);
    b.append(&name);
    b.append(&presence);
    b.append(&badge);
    let row = gtk::ListBoxRow::builder().child(&b).build();

    // Right-click: the buffer's menu.
    let popover = gtk::PopoverMenu::from_model(Some(&buffer_menu(id, &model.name, kind)));
    popover.set_parent(&b);
    popover.set_has_arrow(false);
    let click = gtk::GestureClick::builder().button(3).build();
    let p = popover.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        p.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        p.popup();
    });
    b.add_controller(click);
    b.connect_destroy(move |_| popover.unparent());
    // Middle-click opens it in a new tab.
    b.add_controller(new_tab_on_middle_click(ctrl, id));
    let r = BufferRow {
        row,
        name,
        badge,
        dot: presence,
    };
    update_row(state, id, &r);
    let _ = glib::Propagation::Proceed;
    r
}

fn update_row(state: &AppState, id: &BufferId, row: &BufferRow) {
    let Some(model) = state.buffers.get(id) else {
        return;
    };
    row.name.set_text(&state.buffer_title(id));
    let (unread, mentions) = model.unread_counts();
    if unread > 0 {
        row.name.add_css_class("unread-name");
    } else {
        row.name.remove_css_class("unread-name");
    }
    let count = if mentions > 0 { mentions } else { unread };
    row.badge
        .set_visible(count > 0 && (mentions > 0 || model.kind != BufferKind::Server));
    row.badge.set_text(&if count > 999 {
        "999+".into()
    } else {
        count.to_string()
    });
    if mentions > 0 {
        row.badge.add_css_class("mention-badge");
    } else {
        row.badge.remove_css_class("mention-badge");
    }
    match model.online {
        Some(online) if model.kind == BufferKind::Query => {
            row.dot.set_visible(true);
            set_dot(&row.dot, online, !online);
            row.dot
                .set_tooltip_text(Some(if online { "Online" } else { "Offline" }));
        }
        _ => row.dot.set_visible(false),
    }
}
