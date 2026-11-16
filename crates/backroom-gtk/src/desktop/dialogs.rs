//! Dialogs: join, browse channels, new conversation, WHOIS, search, channel
//! settings, and the network and server editors.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use adw::prelude::*;
use backroom_app::app::{AppState, ChannelSettings};
use backroom_app::composer::{ban_mask, with_hashes};
use backroom_app::servers::{
    Login, NetworkFields, NetworkLogin, NetworkLoginMethod, ServerSettings,
};
use backroom_ffi::{ChannelListing, WhoisInfo};
use chrono::{DateTime, Local};
use gtk::{gio, glib};

use crate::desktop::controller::Controller;

/// Dialogs that follow the state while open.
#[derive(Default)]
pub struct Live {
    browse: RefCell<Option<Browse>>,
    search: RefCell<Option<Search>>,
    settings: RefCell<Option<Rc<ChannelSettingsDialog>>>,
}

impl Live {
    pub fn refresh(&self, state: &AppState, changes: &backroom_app::app::Changes) {
        if let Some(b) = self.browse.borrow().as_ref()
            && changes.channel_lists.contains(&b.network)
        {
            b.refresh(state);
        }
        if changes.search
            && let Some(s) = self.search.borrow().as_ref()
        {
            s.refresh(state);
        }
        if changes.channel_settings
            && let Some(s) = self.settings.borrow().as_ref()
        {
            s.refresh(state.channel_settings.as_ref());
        }
    }
}

fn entry_row(title: &str, text: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).text(text).build()
}

fn act(ctrl: &Rc<Controller>, f: impl FnOnce(&mut AppState) + 'static) {
    ctrl.act(f);
}

// MARK: Join, new conversation

pub fn join_channel(ctrl: &Rc<Controller>, parent: &gtk::Widget, network: String) {
    let dialog = adw::AlertDialog::new(Some("Join Channel"), None);
    let channel = entry_row("Channel", "#");
    let key = adw::PasswordEntryRow::builder()
        .title("Key (if it has one)")
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&channel);
    group.add(&key);
    dialog.set_extra_child(Some(&group));
    dialog.add_responses(&[("cancel", "Cancel"), ("join", "Join")]);
    dialog.set_response_appearance("join", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("join"));
    dialog.set_close_response("cancel");
    let d = dialog.clone();
    channel.connect_entry_activated(move |_| {
        // A typed Enter answers like the button, which also closes.
        d.emit_by_name::<()>("response", &[&"join"]);
        d.close();
    });
    let (c, entry) = (ctrl.weak(), channel.clone());
    dialog.connect_response(Some("join"), move |_, _| {
        let name = entry.text().trim().to_owned();
        if name.is_empty() || name == "#" {
            return;
        }
        let key = Some(key.text().to_string()).filter(|k| !k.is_empty());
        let network = network.clone();
        if let Some(c) = c.upgrade() {
            act(&c, move |s| {
                s.join(&network, &with_hashes(&name), key.as_deref());
            });
        }
    });
    dialog.present(Some(parent));
    channel.grab_focus();
}

pub fn new_conversation(ctrl: &Rc<Controller>, parent: &gtk::Widget, network: String) {
    let dialog = adw::AlertDialog::new(
        Some("New Conversation"),
        Some("Who do you want to talk to?"),
    );
    let nick = entry_row("Nick", "");
    let group = adw::PreferencesGroup::new();
    group.add(&nick);
    dialog.set_extra_child(Some(&group));
    dialog.add_responses(&[("cancel", "Cancel"), ("open", "Open")]);
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("open"));
    dialog.set_close_response("cancel");
    let d = dialog.clone();
    nick.connect_entry_activated(move |_| {
        d.emit_by_name::<()>("response", &[&"open"]);
        d.close();
    });
    let c = ctrl.weak();
    dialog.connect_response(Some("open"), move |_, _| {
        let name = nick.text().trim().to_owned();
        if name.is_empty() || name.contains(' ') {
            return;
        }
        let network = network.clone();
        if let Some(c) = c.upgrade() {
            act(&c, move |s| s.open_query(&network, &name));
        }
    });
    dialog.present(Some(parent));
}

// MARK: Browse channels

struct Browse {
    network: String,
    store: gio::ListStore,
    status: gtk::Label,
    filter: gtk::SearchEntry,
    spinner: gtk::Spinner,
    all: RefCell<Vec<ChannelListing>>,
}

impl Browse {
    fn refresh(&self, state: &AppState) {
        let Some(list) = state.channel_lists.get(&self.network) else {
            return;
        };
        self.spinner.set_spinning(list.loading);
        self.spinner.set_visible(list.loading);
        let mut channels = list.channels.clone();
        channels.sort_by(|a, b| b.users.cmp(&a.users).then(a.name.cmp(&b.name)));
        *self.all.borrow_mut() = channels;
        self.apply_filter();
    }

    fn apply_filter(&self) {
        let needle = self.filter.text().to_lowercase();
        let all = self.all.borrow();
        let shown: Vec<glib::BoxedAnyObject> = all
            .iter()
            .filter(|c| {
                needle.is_empty()
                    || c.name.to_lowercase().contains(&needle)
                    || backroom_ffi::strip_formatting(c.topic.clone())
                        .to_lowercase()
                        .contains(&needle)
            })
            .take(2000)
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.status.set_text(&format!(
            "{} channel{}",
            shown.len(),
            if shown.len() == 1 { "" } else { "s" }
        ));
        self.store.splice(0, self.store.n_items(), &shown);
    }
}

pub fn browse_channels(
    ctrl: &Rc<Controller>,
    live: &Rc<Live>,
    parent: &gtk::Widget,
    network: String,
) {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let name = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let users = gtk::Label::builder()
            .css_classes(["dim-label", "numeric"])
            .build();
        let topic = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .build();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        name.set_hexpand(true);
        top.append(&name);
        top.append(&users);
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.set_margin_top(6);
        b.set_margin_bottom(6);
        b.set_margin_start(6);
        b.set_margin_end(6);
        b.append(&top);
        b.append(&topic);
        item.set_child(Some(&b));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let (Some(obj), Some(b)) = (
            item.item().and_downcast::<glib::BoxedAnyObject>(),
            item.child().and_downcast::<gtk::Box>(),
        ) else {
            return;
        };
        let c = obj.borrow::<ChannelListing>();
        let top = b.first_child().and_downcast::<gtk::Box>().expect("top row");
        let name = top
            .first_child()
            .and_downcast::<gtk::Label>()
            .expect("name");
        let users = top
            .last_child()
            .and_downcast::<gtk::Label>()
            .expect("users");
        let topic = b.last_child().and_downcast::<gtk::Label>().expect("topic");
        name.set_text(&c.name);
        users.set_text(&c.users.to_string());
        let plain = backroom_ffi::strip_formatting(c.topic.clone());
        topic.set_text(&plain);
        topic.set_visible(!plain.is_empty());
        topic.set_tooltip_text(Some(plain.as_str()).filter(|t| !t.is_empty()));
    });
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    let list = gtk::ListView::builder()
        .model(&selection)
        .factory(&factory)
        .single_click_activate(false)
        .css_classes(["navigation-sidebar"])
        .build();
    let filter = gtk::SearchEntry::builder()
        .placeholder_text("Filter by name or topic")
        .hexpand(true)
        .build();
    let spinner = gtk::Spinner::new();
    let status = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&filter);
    top.append(&spinner);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_bottom(12);
    content.append(&top);
    content.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build(),
    );
    content.append(&status);
    let join = gtk::Button::builder()
        .label("Join")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_end(&join);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title(format!(
            "Channels on {}",
            ctrl.read(|s| s.network_name(&network)).unwrap_or_default()
        ))
        .content_width(560)
        .content_height(560)
        .child(&view)
        .build();

    let browse = Browse {
        network: network.clone(),
        store,
        status,
        filter: filter.clone(),
        spinner,
        all: RefCell::default(),
    };
    let weak_live = Rc::downgrade(live);
    filter.connect_search_changed(move |_| {
        if let Some(live) = weak_live.upgrade()
            && let Some(b) = live.browse.borrow().as_ref()
        {
            b.apply_filter();
        }
    });
    let join_selected = {
        let (c, d, selection, network) = (
            ctrl.weak(),
            dialog.clone(),
            selection.clone(),
            network.clone(),
        );
        move || {
            let Some(channel) = selection
                .selected_item()
                .and_downcast::<glib::BoxedAnyObject>()
                .map(|o| o.borrow::<ChannelListing>().name.clone())
            else {
                return;
            };
            if let Some(c) = c.upgrade() {
                let network = network.clone();
                c.act(move |s| {
                    s.join(&network, &channel, None);
                });
            }
            d.close();
        }
    };
    let j = join_selected.clone();
    join.connect_clicked(move |_| j());
    list.connect_activate(move |_, _| join_selected());
    live.browse.replace(Some(browse));
    let weak_live = Rc::downgrade(live);
    dialog.connect_closed(move |_| {
        if let Some(live) = weak_live.upgrade() {
            live.browse.replace(None);
        }
    });
    let n = network.clone();
    ctrl.act(move |s| s.list_channels(&n));
    dialog.present(Some(parent));
    filter.grab_focus();
}

// MARK: WHOIS

fn ago(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h {} min", s / 3600, (s % 3600) / 60),
        _ => format!("{} days", s / 86_400),
    }
}

pub fn whois(ctrl: &Rc<Controller>, parent: &gtk::Widget, network: String, info: WhoisInfo) {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    let row = |title: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let r = adw::ActionRow::builder()
                .title(title)
                .subtitle(glib::markup_escape_text(&value))
                .subtitle_selectable(true)
                .build();
            r.add_css_class("property");
            group.add(&r);
        }
    };
    row("Real name", info.realname.clone());
    row(
        "Address",
        match (&info.user, &info.host) {
            (Some(u), Some(h)) => Some(format!("{u}@{h}")),
            (None, Some(h)) => Some(h.clone()),
            _ => None,
        },
    );
    row("Account", info.account.clone());
    row("Away", info.away.clone());
    row(
        "Server",
        info.server.clone().map(|s| match &info.server_info {
            Some(i) if !i.is_empty() => format!("{s} ({i})"),
            _ => s,
        }),
    );
    row("Idle", info.idle.map(ago));
    row(
        "Connected since",
        info.signon
            .map(|t| DateTime::<Local>::from(t).format("%c").to_string()),
    );
    row(
        "Connection",
        Some(
            [
                info.secure.then_some("secure (TLS)"),
                info.operator.then_some("IRC operator"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", "),
        ),
    );
    row("Channels", Some(info.channels.join(" ")));
    for other in &info.other {
        row("", Some(other.clone()));
    }
    page.add(&group);
    let message = gtk::Button::builder()
        .label("Message")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_start(&message);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(&info.nick)
        .content_width(420)
        .content_height(480)
        .child(&view)
        .build();
    let (c, d, nick) = (ctrl.weak(), dialog.clone(), info.nick.clone());
    message.connect_clicked(move |_| {
        if let Some(c) = c.upgrade() {
            let (network, nick) = (network.clone(), nick.clone());
            c.act(move |s| s.open_query(&network, &nick));
        }
        d.close();
    });
    dialog.present(Some(parent));
}

// MARK: Search

struct Search {
    store: gio::ListStore,
    status: gtk::Label,
}

impl Search {
    fn refresh(&self, state: &AppState) {
        let Some(search) = &state.search else {
            return;
        };
        let objects: Vec<glib::BoxedAnyObject> = search
            .hits
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.store.splice(0, self.store.n_items(), &objects);
        let mut text = if !search.pending.is_empty() {
            "Searching…".to_owned()
        } else if search.hits.is_empty() {
            "Nothing found.".to_owned()
        } else {
            format!(
                "{} result{}",
                search.hits.len(),
                if search.hits.len() == 1 { "" } else { "s" }
            )
        };
        for e in &search.errors {
            text.push_str(&format!("\n{e}"));
        }
        self.status.set_text(&text);
    }
}

pub fn search(
    ctrl: &Rc<Controller>,
    live: &Rc<Live>,
    parent: &gtk::Widget,
    network: String,
    buffer: Option<String>,
) {
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Search messages")
        .hexpand(true)
        .build();
    let scopes: Vec<String> = match &buffer {
        Some(b) => vec![format!("In {b}"), "Everywhere on this network".to_owned()],
        None => vec!["Everywhere on this network".to_owned()],
    };
    let scope = gtk::DropDown::from_strings(&scopes.iter().map(String::as_str).collect::<Vec<_>>());
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&entry);
    top.append(&scope);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let head = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();
        let text = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.set_margin_top(6);
        b.set_margin_bottom(6);
        b.set_margin_start(6);
        b.set_margin_end(6);
        b.append(&head);
        b.append(&text);
        item.set_child(Some(&b));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
        let (Some(obj), Some(b)) = (
            item.item().and_downcast::<glib::BoxedAnyObject>(),
            item.child().and_downcast::<gtk::Box>(),
        ) else {
            return;
        };
        let hit = obj.borrow::<backroom_ffi::SearchHit>();
        let head = b.first_child().and_downcast::<gtk::Label>().expect("head");
        let text = b.last_child().and_downcast::<gtk::Label>().expect("text");
        let when = DateTime::<Local>::from(hit.message.time).format("%-d %b %Y %H:%M");
        head.set_text(&format!("{} · {} · {when}", hit.buffer, hit.message.sender));
        text.set_text(&backroom_ffi::strip_formatting(hit.message.text.clone()));
    });
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    let list = gtk::ListView::builder()
        .model(&selection)
        .factory(&factory)
        .css_classes(["navigation-sidebar"])
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_bottom(12);
    content.append(&top);
    content.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build(),
    );
    content.append(&status);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title("Search")
        .content_width(560)
        .content_height(560)
        .child(&view)
        .build();
    live.search.replace(Some(Search { store, status }));
    let (c, n, b, s) = (ctrl.weak(), network.clone(), buffer.clone(), scope.clone());
    entry.connect_activate(move |e| {
        let text = e.text().to_string();
        let target = if s.selected() == 0 { b.clone() } else { None };
        let network = n.clone();
        if let Some(c) = c.upgrade() {
            c.act(move |st| st.start_search(&network, target.as_deref(), &text));
        }
    });
    let (c, d) = (ctrl.weak(), dialog.clone());
    list.connect_activate(move |list, position| {
        let Some(hit) = list
            .model()
            .and_then(|m| m.item(position))
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|o| o.borrow::<backroom_ffi::SearchHit>().clone())
        else {
            return;
        };
        if let Some(c) = c.upgrade() {
            let network = network.clone();
            c.act(move |s| s.reveal_message(&network, &hit.buffer, &hit.message));
        }
        d.close();
    });
    let weak_live = Rc::downgrade(live);
    dialog.connect_closed(move |_| {
        if let Some(live) = weak_live.upgrade() {
            live.search.replace(None);
        }
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

// MARK: Channel settings

struct ChannelSettingsDialog {
    modes: Vec<(char, adw::SwitchRow)>,
    key: adw::EntryRow,
    limit: adw::SpinRow,
    bans: adw::PreferencesGroup,
    ban_rows: RefCell<Vec<adw::ActionRow>>,
    loaded: RefCell<Option<ChannelSettings>>,
    ctrl: std::rc::Weak<Controller>,
}

const MODES: &[(char, &str, &str)] = &[
    (
        'n',
        "No outside messages",
        "Only members can send to the channel",
    ),
    ('t', "Only operators set the topic", ""),
    (
        'm',
        "Moderated",
        "Only voiced members and operators can talk",
    ),
    ('i', "Invite only", ""),
    ('s', "Secret", "Hidden from the channel list"),
];

impl ChannelSettingsDialog {
    fn refresh(&self, settings: Option<&ChannelSettings>) {
        let Some(settings) = settings else {
            return;
        };
        self.loaded.replace(Some(settings.clone()));
        if let Some(modes) = &settings.modes {
            let (key, limit) = mode_args(modes, &settings.params);
            for (letter, row) in &self.modes {
                row.set_active(modes.contains(*letter));
                row.set_sensitive(true);
            }
            self.key.set_text(key.as_deref().unwrap_or_default());
            self.limit
                .set_value(limit.and_then(|l| l.parse().ok()).unwrap_or(0.0));
        }
        for row in self.ban_rows.borrow_mut().drain(..) {
            self.bans.remove(&row);
        }
        match &settings.bans {
            None => {
                self.bans.set_description(Some("Loading…"));
            }
            Some(bans) => {
                self.bans.set_description(if bans.is_empty() {
                    Some("Nobody is banned.")
                } else {
                    None
                });
                for ban in bans {
                    let subtitle = match (&ban.set_by, ban.set_at) {
                        (Some(by), Some(at)) => format!("by {by}, {}", when(at)),
                        (Some(by), None) => format!("by {by}"),
                        (None, Some(at)) => when(at),
                        (None, None) => String::new(),
                    };
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(&ban.mask))
                        .subtitle(subtitle)
                        .build();
                    let remove = gtk::Button::builder()
                        .icon_name("user-trash-symbolic")
                        .tooltip_text("Remove Ban")
                        .valign(gtk::Align::Center)
                        .css_classes(["flat"])
                        .build();
                    let (c, network, channel, mask) = (
                        self.ctrl.clone(),
                        settings.network.clone(),
                        settings.channel.clone(),
                        ban.mask.clone(),
                    );
                    remove.connect_clicked(move |_| {
                        if let Some(c) = c.upgrade() {
                            let (network, channel, mask) =
                                (network.clone(), channel.clone(), mask.clone());
                            c.act(move |s| {
                                s.send_raw(&network, &format!("MODE {channel} -b {mask}"));
                                s.open_channel_settings(&network, &channel);
                            });
                        }
                    });
                    row.add_suffix(&remove);
                    self.bans.add(&row);
                    self.ban_rows.borrow_mut().push(row);
                }
            }
        }
    }
}

fn when(t: SystemTime) -> String {
    DateTime::<Local>::from(t).format("%-d %b %Y").to_string()
}

/// The key (`+k`) and limit (`+l`) arguments of a mode string.
fn mode_args(modes: &str, params: &[String]) -> (Option<String>, Option<String>) {
    let mut args = params.iter();
    let (mut key, mut limit) = (None, None);
    for letter in modes.chars().filter(|c| c.is_ascii_alphabetic()) {
        match letter {
            'k' => key = args.next().cloned(),
            'l' => limit = args.next().cloned(),
            // Other modes with an argument when set (CHANMODES type B/C).
            'f' | 'j' | 'L' | 'F' | 'H' | 'J' => {
                args.next();
            }
            _ => {}
        }
    }
    (key, limit)
}

pub fn channel_settings(
    ctrl: &Rc<Controller>,
    live: &Rc<Live>,
    parent: &gtk::Widget,
    network: String,
    channel: String,
) {
    let page = adw::PreferencesPage::new();
    let modes_group = adw::PreferencesGroup::builder().title("Modes").build();
    let modes: Vec<(char, adw::SwitchRow)> = MODES
        .iter()
        .map(|(letter, title, subtitle)| {
            let row = adw::SwitchRow::builder()
                .title(*title)
                .subtitle(*subtitle)
                .sensitive(false)
                .build();
            modes_group.add(&row);
            (*letter, row)
        })
        .collect();
    let key = adw::EntryRow::builder()
        .title("Key (password to join)")
        .build();
    let limit = adw::SpinRow::with_range(0.0, 100_000.0, 1.0);
    limit.set_title("User limit");
    limit.set_subtitle("0 for none");
    modes_group.add(&key);
    modes_group.add(&limit);
    page.add(&modes_group);
    let bans = adw::PreferencesGroup::builder().title("Bans").build();
    let add_ban = adw::EntryRow::builder()
        .title("Ban a nick or mask")
        .show_apply_button(true)
        .build();
    bans.add(&add_ban);
    page.add(&bans);

    let apply = gtk::Button::builder()
        .label("Apply")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_end(&apply);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(&channel)
        .content_width(460)
        .content_height(620)
        .child(&view)
        .build();

    let (c, n, ch) = (ctrl.weak(), network.clone(), channel.clone());
    add_ban.connect_apply(move |row| {
        let mask = row.text().trim().to_owned();
        if mask.is_empty() {
            return;
        }
        row.set_text("");
        if let Some(c) = c.upgrade() {
            let (n, ch) = (n.clone(), ch.clone());
            c.act(move |s| {
                s.send_raw(&n, &format!("MODE {ch} +b {}", ban_mask(&mask)));
                s.open_channel_settings(&n, &ch);
            });
        }
    });

    let settings = Rc::new(ChannelSettingsDialog {
        modes,
        key,
        limit,
        bans,
        ban_rows: RefCell::default(),
        loaded: RefCell::default(),
        ctrl: ctrl.weak(),
    });
    let (s, c, d) = (settings.clone(), ctrl.weak(), dialog.clone());
    apply.connect_clicked(move |_| {
        let Some(loaded) = s.loaded.borrow().clone() else {
            return;
        };
        let Some(current) = loaded.modes.clone() else {
            return;
        };
        let (old_key, old_limit) = mode_args(&current, &loaded.params);
        let mut lines = Vec::new();
        let (mut add, mut remove) = (String::new(), String::new());
        for (letter, row) in &s.modes {
            match (current.contains(*letter), row.is_active()) {
                (false, true) => add.push(*letter),
                (true, false) => remove.push(*letter),
                _ => {}
            }
        }
        let ch = &loaded.channel;
        if !add.is_empty() || !remove.is_empty() {
            let mut change = String::new();
            if !add.is_empty() {
                change.push_str(&format!("+{add}"));
            }
            if !remove.is_empty() {
                change.push_str(&format!("-{remove}"));
            }
            lines.push(format!("MODE {ch} {change}"));
        }
        let key = s.key.text().trim().to_owned();
        match (old_key, key.is_empty()) {
            (Some(old), true) => lines.push(format!("MODE {ch} -k {old}")),
            (Some(old), false) if old != key => {
                lines.push(format!("MODE {ch} -k {old}"));
                lines.push(format!("MODE {ch} +k {key}"));
            }
            (None, false) => lines.push(format!("MODE {ch} +k {key}")),
            _ => {}
        }
        let limit = s.limit.value() as u64;
        let old_limit: u64 = old_limit.and_then(|l| l.parse().ok()).unwrap_or(0);
        if limit != old_limit {
            lines.push(if limit == 0 {
                format!("MODE {ch} -l")
            } else {
                format!("MODE {ch} +l {limit}")
            });
        }
        if let Some(c) = c.upgrade() {
            let network = loaded.network.clone();
            c.act(move |st| {
                for line in &lines {
                    st.send_raw(&network, line);
                }
            });
        }
        d.close();
    });
    live.settings.replace(Some(settings.clone()));
    let weak_live = Rc::downgrade(live);
    dialog.connect_closed(move |_| {
        if let Some(live) = weak_live.upgrade() {
            live.settings.replace(None);
        }
    });
    let (n, ch) = (network.clone(), channel.clone());
    ctrl.act(move |s| s.open_channel_settings(&n, &ch));
    dialog.present(Some(parent));
}

// MARK: Networks

/// Adds a network (`id` `None`) or edits one.
pub fn network_form(ctrl: &Rc<Controller>, parent: &gtk::Widget, id: Option<String>) {
    let (fields, standalone, name) = ctrl
        .read(|s| {
            let standalone = s.is_standalone();
            let fields = id
                .as_ref()
                .and_then(|id| s.network(id))
                .map(|n| NetworkFields::from_attributes(&n.network.attributes, standalone))
                .unwrap_or_else(|| NetworkFields {
                    port: 6697,
                    tls: true,
                    login: standalone.then(NetworkLogin::default),
                    ..NetworkFields::default()
                });
            let name = id.as_ref().map(|id| s.network_name(id));
            (fields, standalone, name)
        })
        .unwrap_or_default();
    let saved_login = fields.login.clone();
    let page = adw::PreferencesPage::new();
    let server = adw::PreferencesGroup::builder().title("Server").build();
    let host = entry_row("Address", &fields.host);
    let port = adw::SpinRow::with_range(1.0, 65_535.0, 1.0);
    port.set_title("Port");
    port.set_value(f64::from(fields.port));
    let tls = adw::SwitchRow::builder()
        .title("Use TLS")
        .subtitle("Encrypted connection; most networks use port 6697")
        .active(fields.tls)
        .build();
    let p2 = port.clone();
    tls.connect_active_notify(move |t| {
        let v = p2.value() as u16;
        if t.is_active() && v == 6667 {
            p2.set_value(6697.0);
        } else if !t.is_active() && v == 6697 {
            p2.set_value(6667.0);
        }
    });
    server.add(&host);
    server.add(&port);
    server.add(&tls);
    page.add(&server);
    let you = adw::PreferencesGroup::builder().title("You").build();
    let display = entry_row("Name (optional)", &fields.name);
    let nick = entry_row("Nick (optional)", &fields.nick);
    you.add(&display);
    you.add(&nick);
    page.add(&you);

    let login_rows = standalone.then(|| {
        let group = adw::PreferencesGroup::builder()
            .title("Login")
            .description("Passwords are kept in your keyring.")
            .build();
        let titles: Vec<&str> = NetworkLoginMethod::ALL.iter().map(|m| m.title()).collect();
        let method = adw::ComboRow::builder()
            .title("Method")
            .model(&gtk::StringList::new(&titles))
            .build();
        let current = fields.login.clone().unwrap_or_default();
        method.set_selected(
            NetworkLoginMethod::ALL
                .iter()
                .position(|m| *m == current.method)
                .unwrap_or(0) as u32,
        );
        let username = entry_row("Account (the nick if empty)", &current.username);
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        group.add(&method);
        group.add(&username);
        group.add(&password);
        let update = {
            let (u, p) = (username.clone(), password.clone());
            let saved = saved_login.clone();
            move |m: &adw::ComboRow| {
                let method = NetworkLoginMethod::ALL[m.selected() as usize];
                u.set_visible(method == NetworkLoginMethod::Sasl);
                p.set_visible(method != NetworkLoginMethod::None);
                let saved_same = saved.as_ref().is_some_and(|s| s.method == method);
                p.set_title(if saved_same {
                    "Password (empty keeps the saved one)"
                } else {
                    "Password"
                });
            }
        };
        update(&method);
        method.connect_selected_notify(update);
        page.add(&group);
        (method, username, password)
    });

    let save = gtk::Button::builder()
        .label(if id.is_some() { "Save" } else { "Add" })
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    let cancel = gtk::Button::with_label("Cancel");
    header.pack_start(&cancel);
    header.pack_end(&save);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(name.map_or_else(|| "Add Network".to_owned(), |n| format!("Edit {n}")))
        .content_width(460)
        .content_height(if standalone { 640 } else { 520 })
        .child(&view)
        .build();
    let d = dialog.clone();
    cancel.connect_clicked(move |_| {
        d.close();
    });
    let (c, d) = (ctrl.weak(), dialog.clone());
    save.connect_clicked(move |_| {
        let host = host.text().trim().to_owned();
        if host.is_empty() {
            host_error(&d);
            return;
        }
        let login = login_rows.as_ref().map(|(m, u, p)| NetworkLogin {
            method: NetworkLoginMethod::ALL[m.selected() as usize],
            username: u.text().trim().to_owned(),
            password: p.text().to_string(),
        });
        if let Some(l) = &login
            && !l.is_complete(saved_login.as_ref())
        {
            let toast = adw::AlertDialog::new(
                Some("Password needed"),
                Some("Type the password for this login method."),
            );
            toast.add_response("ok", "OK");
            toast.present(Some(&d));
            return;
        }
        let fields = NetworkFields {
            name: display.text().trim().to_owned(),
            host,
            port: port.value() as u16,
            tls: tls.is_active(),
            nick: nick.text().trim().to_owned(),
            login,
        };
        if let Some(c) = c.upgrade() {
            let id = id.clone();
            c.act(move |s| match &id {
                Some(id) => s.change_network(id, &fields),
                None => s.add_network(&fields),
            });
        }
        d.close();
    });
    dialog.present(Some(parent));
}

fn host_error(parent: &adw::Dialog) {
    let alert = adw::AlertDialog::new(
        Some("Address needed"),
        Some("Type the server's address, e.g. irc.libera.chat."),
    );
    alert.add_response("ok", "OK");
    alert.present(Some(parent));
}

pub fn remove_network(ctrl: &Rc<Controller>, parent: &gtk::Widget, id: String) {
    let (name, message) = ctrl
        .read(|s| (s.network_name(&id), s.remove_network_message().to_owned()))
        .unwrap_or_default();
    let dialog = adw::AlertDialog::new(Some(&format!("Remove {name}?")), Some(&message));
    dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let c = ctrl.weak();
    dialog.connect_response(Some("remove"), move |_, _| {
        if let Some(c) = c.upgrade() {
            let id = id.clone();
            c.act(move |s| s.remove_network(&id));
        }
    });
    dialog.present(Some(parent));
}

// MARK: Servers

/// Adds a soju server (`server` `None`) or edits one.
pub fn server_editor(ctrl: &Rc<Controller>, parent: &gtk::Widget, server: Option<ServerSettings>) {
    let editing = server.is_some();
    let server = server.unwrap_or_else(ServerSettings::blank);
    let standalone = server.is_standalone();
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    let name = entry_row("Name (optional)", &server.name);
    let url = entry_row("Address", &server.server_url);
    let methods = ["Username and password", "Single sign-on (browser)"];
    let login = adw::ComboRow::builder()
        .title("Login")
        .model(&gtk::StringList::new(&methods))
        .selected(u32::from(server.login == Login::SingleSignOn))
        .build();
    let username = entry_row("Username", &server.username);
    let issuer = entry_row("Login server (OIDC issuer)", &server.issuer);
    let nick = entry_row(
        if standalone {
            "Default nick for networks"
        } else {
            "Nick (optional)"
        },
        &server.nick,
    );
    if !standalone {
        group.set_description(Some(
            "The soju address is usually wss://host/socket or ircs://host:6697.",
        ));
        group.add(&name);
        group.add(&url);
        group.add(&login);
        group.add(&username);
        group.add(&issuer);
    }
    group.add(&nick);
    page.add(&group);
    let update = {
        let (u, i) = (username.clone(), issuer.clone());
        move |l: &adw::ComboRow| {
            u.set_visible(l.selected() == 0);
            i.set_visible(l.selected() == 1);
        }
    };
    update(&login);
    login.connect_selected_notify(update);

    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label(if editing { "Save" } else { "Add" })
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    if editing && !standalone {
        let remove = gtk::Button::builder()
            .label("Remove Server")
            .halign(gtk::Align::Center)
            .css_classes(["destructive-action", "pill"])
            .margin_bottom(18)
            .build();
        view.add_bottom_bar(&remove);
        let (c, d, id) = (ctrl.weak(), parent.clone(), server.id);
        remove.connect_clicked(move |b| {
            let alert = adw::AlertDialog::new(
                Some("Remove this server?"),
                Some("Its saved login is deleted too."),
            );
            alert.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
            alert.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            alert.set_close_response("cancel");
            let (c, dialog) = (
                c.clone(),
                b.ancestor(adw::Dialog::static_type())
                    .and_downcast::<adw::Dialog>(),
            );
            alert.connect_response(Some("remove"), move |_, _| {
                if let Some(c) = c.upgrade() {
                    c.remove_server(id);
                }
                if let Some(d) = &dialog {
                    d.close();
                }
            });
            alert.present(Some(&d));
        });
    }
    let dialog = adw::Dialog::builder()
        .title(if standalone {
            "No Bouncer".to_owned()
        } else if editing {
            "Edit Server".to_owned()
        } else {
            "Add soju Server".to_owned()
        })
        .content_width(480)
        .content_height(if standalone { 260 } else { 520 })
        .child(&view)
        .build();
    let d = dialog.clone();
    cancel.connect_clicked(move |_| {
        d.close();
    });
    let (c, d) = (ctrl.weak(), dialog.clone());
    save.connect_clicked(move |_| {
        let mut updated = server.clone();
        updated.nick = nick.text().trim().to_owned();
        if !standalone {
            updated.name = name.text().trim().to_owned();
            updated.server_url = url.text().trim().to_owned();
            updated.login = if login.selected() == 1 {
                Login::SingleSignOn
            } else {
                Login::Password
            };
            updated.username = username.text().trim().to_owned();
            updated.issuer = issuer.text().trim().to_owned();
            if !updated.is_complete() {
                let alert = adw::AlertDialog::new(
                    Some("Missing details"),
                    Some(if updated.login == Login::Password {
                        "Fill in the address and the username."
                    } else {
                        "Fill in the address and the login server."
                    }),
                );
                alert.add_response("ok", "OK");
                alert.present(Some(&d));
                return;
            }
        }
        if let Some(c) = c.upgrade() {
            c.save_server(updated);
        }
        d.close();
    });
    dialog.present(Some(parent));
}
