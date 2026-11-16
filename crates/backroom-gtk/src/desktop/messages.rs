//! One pane: a conversation's topic bar, message list, typing line and
//! composer.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime};

use adw::prelude::*;
use backroom_app::app::{AppState, is_event};
use backroom_app::buffer::{BufferId, BufferModel, connection_summary, stable_id};
use backroom_app::composer::Completion;
use backroom_app::mentions::MentionMatcher;
use backroom_app::prefs::{MessageLayout, NickColoring, Preferences, TimestampStyle};
use backroom_app::timeline::{self, TimelineItem};
use backroom_ffi::{BufferKind, ChatMessage, ConnectionState, MessageKind, TypingState};
use chrono::{DateTime, Local};
use gtk::{gdk, gio, glib};
use uuid::Uuid;

use crate::desktop::controller::Controller;
use crate::desktop::render::{self, channel_of};
use crate::desktop::style::Palette;

/// What one line of the list shows.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    /// Above the oldest message: load more, loading, or the beginning.
    Header(HeaderState),
    Item(TimelineItem),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderState {
    More,
    Loading,
    Beginning,
    Cleared,
}

/// Everything rows need, worked out once per redraw rather than per row.
pub struct RowCtx {
    pub layout: MessageLayout,
    pub timestamps: TimestampStyle,
    pub repeat_nick: bool,
    pub nick_width: u32,
    pub coloring: NickColoring,
    pub formatting: bool,
    pub link_previews: bool,
    pub palette: Palette,
    network: String,
    kind: BufferKind,
    prefixes: HashMap<String, String>,
    mentions: MentionMatcher,
    flash: Option<String>,
}

impl RowCtx {
    fn new(prefs: &Preferences, palette: Palette) -> Self {
        RowCtx {
            layout: prefs.layout,
            timestamps: prefs.timestamps,
            repeat_nick: prefs.repeat_nick,
            nick_width: prefs.nick_column_width,
            coloring: prefs.nick_coloring,
            formatting: prefs.show_formatting,
            link_previews: prefs.link_previews,
            palette,
            network: String::new(),
            kind: BufferKind::Server,
            prefixes: HashMap::new(),
            mentions: MentionMatcher::default(),
            flash: None,
        }
    }

    fn prefix(&self, nick: &str) -> Option<&str> {
        self.prefixes.get(&nick.to_lowercase()).map(String::as_str)
    }

    fn nick_color(&self, m: &ChatMessage) -> Option<String> {
        if m.own {
            return Some(self.palette.accent.clone());
        }
        match self.coloring {
            NickColoring::Rank => self
                .palette
                .rank(self.prefix(&m.sender).and_then(|p| p.chars().next()))
                .map(str::to_owned),
            NickColoring::Nick => Some(self.palette.nick(&m.sender).to_owned()),
            NickColoring::None => self.palette.text.clone(),
        }
    }

    /// The nick with its highest prefix in front, IRC style: `@alice`.
    fn nick_label(&self, nick: &str) -> String {
        let prefix = self
            .prefix(nick)
            .and_then(|p| p.chars().next())
            .map(String::from)
            .unwrap_or_default();
        format!("{prefix}{nick}")
    }

    fn time(&self, time: SystemTime) -> String {
        let local = DateTime::<Local>::from(time);
        match self.timestamps {
            TimestampStyle::Hidden => String::new(),
            TimestampStyle::Minutes => local.format("%H:%M").to_string(),
            TimestampStyle::Seconds => local.format("%H:%M:%S").to_string(),
        }
    }
}

struct Inner {
    id: Uuid,
    ctrl: Weak<Controller>,
    root: gtk::Box,
    stack: gtk::Stack,
    title: gtk::Label,
    subtitle: gtk::Label,
    topic: gtk::Label,
    scroller: gtk::ScrolledWindow,
    list: gtk::ListView,
    store: gio::ListStore,
    rows: RefCell<Vec<Row>>,
    ctx: Rc<RefCell<RowCtx>>,
    typing: gtk::Label,
    composer: gtk::TextView,
    placeholder: gtk::Label,
    buffer: RefCell<Option<BufferId>>,
    /// Keep the newest message in view as more arrive.
    stick: Cell<bool>,
    focused: Cell<bool>,
    completion: RefCell<Option<Completion>>,
    history_index: Cell<Option<usize>>,
    typing_sent: Cell<Option<Instant>>,
    typing_pause: RefCell<Option<glib::SourceId>>,
    typing_tick: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct BufferPane(Rc<Inner>);

impl BufferPane {
    pub fn new(ctrl: &Rc<Controller>, id: Uuid, prefs: &Preferences, palette: Palette) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("pane");

        // Topic bar: title, state or topic, and the pane's menu.
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build();
        let subtitle = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["muted", "caption"])
            .build();
        let topic = gtk::Label::builder()
            .xalign(0.0)
            .use_markup(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(false)
            .lines(1)
            .hexpand(true)
            .tooltip_text("Click to show the whole topic")
            .build();
        let titles = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        titles.append(&title);
        titles.append(&subtitle);
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
        heading.set_hexpand(true);
        heading.append(&titles);
        heading.append(&topic);
        let menu = gio::Menu::new();
        let panes = gio::Menu::new();
        panes.append(Some("Split Right"), Some("app.split-right"));
        panes.append(Some("Split Down"), Some("app.split-down"));
        panes.append(Some("Close Pane"), Some("app.close-pane"));
        menu.append_section(None, &panes);
        let buffer_menu = gio::Menu::new();
        buffer_menu.append(Some("Search…"), Some("app.search-current"));
        buffer_menu.append(
            Some("Channel Settings…"),
            Some("app.channel-settings-current"),
        );
        buffer_menu.append(Some("Leave or Close"), Some("app.close-buffer-current"));
        menu.append_section(None, &buffer_menu);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text("Pane")
            .css_classes(["flat"])
            .build();
        let topic_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        topic_bar.add_css_class("topic-bar");
        topic_bar.append(&heading);
        topic_bar.append(&menu_button);

        // Messages.
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let ctx = Rc::new(RefCell::new(RowCtx::new(prefs, palette)));
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["chat-view"])
            .build();
        let typing = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["typing"])
            .visible(false)
            .build();

        // Composer.
        let composer = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(12)
            .right_margin(12)
            .css_classes(["composer", "chat-text"])
            .build();
        let placeholder = gtk::Label::builder()
            .xalign(0.0)
            .valign(gtk::Align::Start)
            .margin_start(12)
            .margin_top(8)
            .css_classes(["dim-label", "chat-text"])
            .can_target(false)
            .build();
        let composer_scroll = gtk::ScrolledWindow::builder()
            .child(&composer)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(140)
            .build();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&composer_scroll));
        overlay.add_overlay(&placeholder);
        let composer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        composer_box.add_css_class("composer-box");
        composer_box.append(&overlay);

        let chat = gtk::Box::new(gtk::Orientation::Vertical, 0);
        chat.append(&topic_bar);
        chat.append(&scroller);
        chat.append(&typing);
        chat.append(&composer_box);

        let empty = adw::StatusPage::builder()
            .icon_name("user-available-symbolic")
            .title("No Conversation")
            .description("Pick a channel or conversation in the sidebar.")
            .build();
        empty.add_css_class("compact");
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&chat, Some("chat"));
        stack.set_vexpand(true);
        root.append(&stack);

        let pane = BufferPane(Rc::new(Inner {
            id,
            ctrl: ctrl.weak(),
            root,
            stack,
            title,
            subtitle,
            topic,
            scroller,
            list,
            store,
            rows: RefCell::new(Vec::new()),
            ctx,
            typing,
            composer,
            placeholder,
            buffer: RefCell::new(None),
            stick: Cell::new(true),
            focused: Cell::new(false),
            completion: RefCell::new(None),
            history_index: Cell::new(None),
            typing_sent: Cell::new(None),
            typing_pause: RefCell::new(None),
            typing_tick: RefCell::new(None),
        }));
        pane.setup_factory(&factory);
        pane.connect_signals(&menu_button);
        pane
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.0.root
    }

    pub fn buffer(&self) -> Option<BufferId> {
        self.0.buffer.borrow().clone()
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn setup_factory(&self, factory: &gtk::SignalListItemFactory) {
        let ctx = self.0.ctx.clone();
        let ctrl = self.0.ctrl.clone();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            item.set_activatable(false);
            item.set_focusable(false);
            item.set_child(Some(&adw::Bin::new()));
        });
        let weak = self.weak();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let (Some(obj), Some(bin)) = (
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                item.child().and_downcast::<adw::Bin>(),
            ) else {
                return;
            };
            let row = obj.borrow::<Row>();
            let widget = build_row(&ctx.borrow(), &row, &ctrl, &weak);
            bin.set_child(Some(&widget));
        });
        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            if let Some(bin) = item.child().and_downcast::<adw::Bin>() {
                bin.set_child(gtk::Widget::NONE);
            }
        });
    }

    fn connect_signals(&self, menu_button: &gtk::MenuButton) {
        let inner = &self.0;
        // Clicking anywhere in a pane focuses it.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(p) = weak.upgrade() {
                focus_pane(&p);
            }
        });
        inner.root.add_controller(click);
        let weak = self.weak();
        menu_button.connect_active_notify(move |_| {
            if let Some(p) = weak.upgrade() {
                focus_pane(&p);
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let weak = self.weak();
        focus.connect_enter(move |_| {
            if let Some(p) = weak.upgrade() {
                focus_pane(&p);
            }
        });
        inner.composer.add_controller(focus);

        // The whole topic on click.
        let topic_click = gtk::GestureClick::new();
        let topic = inner.topic.clone();
        topic_click.connect_released(move |gesture, _, _, _| {
            if topic.current_uri().is_some() {
                return;
            }
            let wrapped = topic.lines() == -1;
            topic.set_lines(if wrapped { 1 } else { -1 });
            topic.set_wrap(!wrapped);
            topic.set_ellipsize(if wrapped {
                gtk::pango::EllipsizeMode::End
            } else {
                gtk::pango::EllipsizeMode::None
            });
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        inner.topic.add_controller(topic_click);
        connect_links(&inner.topic, &inner.ctrl, &self.weak(), false);

        // Scrolling: follow new messages at the bottom, load more at the top.
        let adjustment = inner.scroller.vadjustment();
        let weak = self.weak();
        adjustment.connect_value_changed(move |adj| {
            let Some(p) = weak.upgrade() else {
                return;
            };
            p.stick
                .set(adj.value() >= adj.upper() - adj.page_size() - 40.0);
            if adj.value() < 300.0 && adj.upper() > adj.page_size() {
                load_older(&p);
            }
        });

        // Composer keys: Enter sends (Shift+Enter is a new line), Tab
        // completes, ↑/↓ step through sent lines.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(p) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let plain = !mods.intersects(
                gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK,
            );
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter if plain => {
                    submit(&p);
                    glib::Propagation::Stop
                }
                gdk::Key::Tab if plain => {
                    complete(&p);
                    glib::Propagation::Stop
                }
                gdk::Key::Up if plain && step_history(&p, true) => glib::Propagation::Stop,
                gdk::Key::Down if plain && step_history(&p, false) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        inner.composer.add_controller(keys);
        let weak = self.weak();
        inner.composer.buffer().connect_changed(move |b| {
            let Some(p) = weak.upgrade() else {
                return;
            };
            let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            p.placeholder.set_visible(text.is_empty());
            let stale = p
                .completion
                .borrow()
                .as_ref()
                .is_some_and(|c| c.result != text);
            if stale {
                p.completion.replace(None);
            }
            typing(&p, &text);
        });
    }

    /// Shows the pane's buffer, redrawing what changed.
    pub fn refresh(&self, state: &AppState, buffer: Option<&BufferId>, focused: bool) {
        let inner = &self.0;
        let switched = inner.buffer.borrow().as_ref() != buffer;
        if switched {
            stop_typing(inner);
            inner.buffer.replace(buffer.cloned());
            inner.stick.set(true);
            inner.completion.replace(None);
            inner.history_index.set(None);
            inner.composer.buffer().set_text("");
            inner.topic.set_lines(1);
            inner.topic.set_wrap(false);
            inner.topic.set_ellipsize(gtk::pango::EllipsizeMode::End);
        }
        if focused != inner.focused.replace(focused) || switched {
            if focused {
                inner.root.add_css_class("focused-pane");
            } else {
                inner.root.remove_css_class("focused-pane");
            }
            if focused && inner.composer.is_mapped() && !inner.composer.has_focus() {
                inner.composer.grab_focus();
            }
        }
        let model = buffer.and_then(|b| state.buffers.get(b));
        let Some(model) = model else {
            inner.stack.set_visible_child_name("empty");
            if switched {
                inner.rows.replace(Vec::new());
                inner.store.remove_all();
            }
            return;
        };
        inner.stack.set_visible_child_name("chat");
        self.refresh_header(state, model);
        self.refresh_typing(model);
        // A new buffer, or one that only now exists (a pane from last time).
        if switched || inner.rows.borrow().is_empty() {
            self.update_ctx(state, model);
            self.refresh_rows(state, model, true);
        }
    }

    /// The buffer changed: its messages, and/or (`details`) its topic,
    /// members, typing or state. One update can bring both (a QUIT is a
    /// message and a member leaving).
    pub fn buffer_changed(&self, state: &AppState, details: bool) {
        let Some(model) = self.buffer().and_then(|b| state.buffers.get(&b)) else {
            return;
        };
        if details {
            self.refresh_header(state, model);
            self.refresh_typing(model);
        }
        self.update_ctx(state, model);
        // The header row follows the history's state.
        self.refresh_rows(state, model, false);
    }

    /// Preferences or theme changed: rebuild every row.
    pub fn restyle(&self, state: &AppState, palette: Palette) {
        *self.0.ctx.borrow_mut() = RowCtx::new(&state.prefs, palette);
        if let Some(model) = self.buffer().and_then(|b| state.buffers.get(&b)) {
            self.update_ctx(state, model);
            self.refresh_rows(state, model, true);
            self.refresh_header(state, model);
        }
    }

    fn update_ctx(&self, state: &AppState, model: &BufferModel) {
        let mut ctx = self.0.ctx.borrow_mut();
        ctx.network.clone_from(&model.id.network);
        ctx.kind = model.kind;
        ctx.prefixes = model
            .members()
            .iter()
            .map(|m| (m.nick.to_lowercase(), m.prefix.clone()))
            .collect();
        ctx.mentions = model.mentions.clone();
        let _ = state;
    }

    fn refresh_header(&self, state: &AppState, model: &BufferModel) {
        let inner = &self.0;
        let network = state.network(&model.id.network);
        inner.title.set_text(&state.buffer_title(&model.id));
        let subtitle = match model.kind {
            BufferKind::Server => network
                .map(|n| connection_summary(&n.connection))
                .unwrap_or_default(),
            BufferKind::Query => match model.online {
                Some(true) => "online".into(),
                Some(false) => "offline".into(),
                None => String::new(),
            },
            BufferKind::Channel => {
                let n = model.members().len();
                if n == 0 {
                    String::new()
                } else if n == 1 {
                    "1 member".into()
                } else {
                    format!("{n} members")
                }
            }
        };
        inner.subtitle.set_text(&subtitle);
        let topic_markup = match (&model.topic, model.kind) {
            (Some(topic), _) if !topic.trim().is_empty() => {
                let ctx = inner.ctx.borrow();
                render::message_markup(topic, ctx.formatting, &ctx.palette, false)
            }
            (_, BufferKind::Server) => match network.map(|n| &n.connection) {
                Some(ConnectionState::Connected) | None => String::new(),
                Some(state) => glib::markup_escape_text(&connection_summary(state)).to_string(),
            },
            _ => String::new(),
        };
        inner.topic.set_markup(&topic_markup);
        inner.topic.set_visible(!topic_markup.is_empty());
        let placeholder = match model.kind {
            BufferKind::Server => "Commands, e.g. /join #channel".to_owned(),
            _ => format!("Message {}", model.name),
        };
        inner.placeholder.set_text(&placeholder);
    }

    fn refresh_typing(&self, model: &BufferModel) {
        let inner = &self.0;
        let nicks = model.typing_now(SystemTime::now());
        let text = match nicks.len() {
            0 => String::new(),
            1 => format!("{} is typing…", nicks[0]),
            2 => format!("{} and {} are typing…", nicks[0], nicks[1]),
            n => format!("{n} people are typing…"),
        };
        inner.typing.set_text(&text);
        inner.typing.set_visible(!nicks.is_empty());
        // Re-checked every second so notifications expire.
        let ticking = inner.typing_tick.borrow().is_some();
        if !nicks.is_empty() && !ticking {
            let weak = self.weak();
            let id = glib::timeout_add_local(Duration::from_secs(1), move || {
                let Some(p) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let pane = BufferPane(p.clone());
                let still = p
                    .ctrl
                    .upgrade()
                    .and_then(|c| {
                        c.read(|s| {
                            let model = pane.buffer().and_then(|b| s.buffers.get(&b))?;
                            pane.refresh_typing_text(model);
                            Some(!model.typing_now(SystemTime::now()).is_empty())
                        })
                    })
                    .flatten()
                    .unwrap_or(false);
                if still {
                    glib::ControlFlow::Continue
                } else {
                    p.typing_tick.replace(None);
                    glib::ControlFlow::Break
                }
            });
            inner.typing_tick.replace(Some(id));
        }
    }

    fn refresh_typing_text(&self, model: &BufferModel) {
        let nicks = model.typing_now(SystemTime::now());
        let text = match nicks.len() {
            0 => String::new(),
            1 => format!("{} is typing…", nicks[0]),
            2 => format!("{} and {} are typing…", nicks[0], nicks[1]),
            n => format!("{n} people are typing…"),
        };
        self.0.typing.set_text(&text);
        self.0.typing.set_visible(!nicks.is_empty());
    }

    fn refresh_rows(&self, state: &AppState, model: &BufferModel, rebind: bool) {
        let inner = &self.0;
        let header = if model.cleared {
            HeaderState::Cleared
        } else if model.history_exhausted {
            HeaderState::Beginning
        } else if model.history_loading || !model.history_requested {
            HeaderState::Loading
        } else {
            HeaderState::More
        };
        let mentions = model.mentions.clone();
        let items = timeline::build(
            model.messages(),
            state.prefs.presence,
            model.unread_line,
            &|nick| mentions.is_ignored(nick),
            &timeline::local_day,
        );
        let new: Vec<Row> = std::iter::once(Row::Header(header))
            .chain(items.into_iter().map(Row::Item))
            .collect();
        let mut old = inner.rows.borrow_mut();
        if rebind {
            let objects: Vec<glib::BoxedAnyObject> =
                new.iter().cloned().map(glib::BoxedAnyObject::new).collect();
            inner.store.splice(0, inner.store.n_items(), &objects);
        } else {
            // Only the changed middle: appends and prepends keep the rest.
            let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            let max_suffix = old.len().min(new.len()) - prefix;
            let suffix = old
                .iter()
                .rev()
                .zip(new.iter().rev())
                .take(max_suffix)
                .take_while(|(a, b)| a == b)
                .count();
            let removed = old.len() - prefix - suffix;
            let added: Vec<glib::BoxedAnyObject> = new[prefix..new.len() - suffix]
                .iter()
                .cloned()
                .map(glib::BoxedAnyObject::new)
                .collect();
            if removed > 0 || !added.is_empty() {
                inner.store.splice(prefix as u32, removed as u32, &added);
            }
        }
        *old = new;
        let count = old.len();
        drop(old);
        if inner.stick.get() && count > 1 {
            scroll_to_end(inner);
        }
        // Too few messages to scroll: fetch more until the view fills.
        if header == HeaderState::More {
            let weak = self.weak();
            glib::timeout_add_local_once(Duration::from_millis(50), move || {
                if let Some(p) = weak.upgrade() {
                    let adj = p.scroller.vadjustment();
                    if adj.upper() <= adj.page_size() + 1.0 {
                        load_older(&p);
                    }
                }
            });
        }
    }

    /// Scrolls to a message (from search) and flashes it.
    pub fn scroll_to(&self, message: &str) {
        let inner = &self.0;
        let index = inner.rows.borrow().iter().position(|r| match r {
            Row::Item(TimelineItem::Message { message: m, .. }) => stable_id(m) == message,
            _ => false,
        });
        let Some(index) = index else {
            return;
        };
        inner.stick.set(false);
        inner.ctx.borrow_mut().flash = Some(message.to_owned());
        inner.store.items_changed(index as u32, 1, 1);
        inner
            .list
            .scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
        let weak = self.weak();
        glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(p) = weak.upgrade() {
                p.ctx.borrow_mut().flash = None;
                if (index as u32) < p.store.n_items() {
                    p.store.items_changed(index as u32, 1, 1);
                }
            }
        });
    }

    pub fn focus_composer(&self) {
        self.0.composer.grab_focus();
    }
}

fn scroll_to_end(inner: &Inner) {
    let list = inner.list.clone();
    let store = inner.store.clone();
    // After the list has measured the new rows.
    glib::idle_add_local_once(move || {
        let n = store.n_items();
        if n > 0 {
            list.scroll_to(n - 1, gtk::ListScrollFlags::NONE, None);
        }
    });
}

fn focus_pane(p: &Rc<Inner>) {
    if p.focused.get() {
        return;
    }
    let id = p.id;
    if let Some(c) = p.ctrl.upgrade() {
        c.act(move |s| s.focus(id));
    }
}

fn load_older(p: &Rc<Inner>) {
    let Some(id) = p.buffer.borrow().clone() else {
        return;
    };
    let Some(c) = p.ctrl.upgrade() else {
        return;
    };
    let ready = c
        .read(|s| {
            s.buffers.get(&id).is_some_and(|b| {
                b.history_requested && !b.history_loading && !b.history_exhausted && !b.cleared
            })
        })
        .unwrap_or(false);
    if ready {
        c.act(move |s| s.load_older(&id));
    }
}

fn composer_text(p: &Inner) -> String {
    let b = p.composer.buffer();
    b.text(&b.start_iter(), &b.end_iter(), false).to_string()
}

fn set_composer_text(p: &Inner, text: &str) {
    let b = p.composer.buffer();
    b.set_text(text);
    b.place_cursor(&b.end_iter());
}

fn submit(p: &Rc<Inner>) {
    let Some(id) = p.buffer.borrow().clone() else {
        return;
    };
    let Some(c) = p.ctrl.upgrade() else {
        return;
    };
    let line = composer_text(p);
    let weak = Rc::downgrade(p);
    c.act(move |s| {
        if s.send(&id, &line) {
            s.remember_sent(&id, &line);
            if let Some(p) = weak.upgrade() {
                p.history_index.set(None);
                stop_typing(&p);
                p.stick.set(true);
                set_composer_text(&p, "");
            }
        }
    });
}

/// Tab: completes the last word with a nick, channel or command; again for
/// the next match.
fn complete(p: &Rc<Inner>) {
    let Some(id) = p.buffer.borrow().clone() else {
        return;
    };
    let text = composer_text(p);
    let mut current = p.completion.borrow_mut();
    if let Some(c) = current.as_mut()
        && c.result == text
    {
        c.advance();
        let next = c.result.clone();
        drop(current);
        set_composer_text(p, &next);
        return;
    }
    let Some(ctrl) = p.ctrl.upgrade() else {
        return;
    };
    let started = ctrl
        .read(|s| {
            Completion::start(
                &text,
                &s.completion_nicks(&id),
                &s.completion_channels(&id.network),
            )
        })
        .flatten();
    if let Some(started) = started {
        let line = started.result.clone();
        *current = Some(started);
        drop(current);
        set_composer_text(p, &line);
    }
}

/// ↑/↓ through lines sent here, when the composer is empty or showing one
/// of them. Returns false to let the key move the cursor instead.
fn step_history(p: &Rc<Inner>, back: bool) -> bool {
    let Some(id) = p.buffer.borrow().clone() else {
        return false;
    };
    let Some(history) = p
        .ctrl
        .upgrade()
        .and_then(|c| c.read(|s| s.buffers.get(&id).map(|b| b.sent_history.clone())))
        .flatten()
    else {
        return false;
    };
    let draft = composer_text(p);
    if history.is_empty() || draft.contains('\n') {
        return false;
    }
    match p.history_index.get() {
        Some(i) if history.get(i).is_some_and(|h| h != &draft) => return false,
        None if !draft.is_empty() => return false,
        _ => {}
    }
    let current = p.history_index.get().unwrap_or(history.len()) as isize;
    let next = if back { current - 1 } else { current + 1 };
    if next < 0 {
        return true;
    }
    if next as usize >= history.len() {
        p.history_index.set(None);
        set_composer_text(p, "");
    } else {
        p.history_index.set(Some(next as usize));
        set_composer_text(p, &history[next as usize]);
    }
    true
}

/// Typing notifications: active at most every 3 s while typing, paused
/// after 6 s without a keystroke, done when the line is cleared or sent.
fn typing(p: &Rc<Inner>, text: &str) {
    let Some(id) = p.buffer.borrow().clone() else {
        return;
    };
    let Some(c) = p.ctrl.upgrade() else {
        return;
    };
    let wanted = c
        .read(|s| {
            s.prefs.send_typing
                && s.buffers
                    .get(&id)
                    .is_some_and(|b| b.kind != BufferKind::Server)
        })
        .unwrap_or(false);
    if !wanted {
        return;
    }
    if text.is_empty() || text.starts_with('/') {
        stop_typing(p);
        return;
    }
    if p.typing_sent
        .get()
        .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
    {
        let target = id.clone();
        c.act(move |s| s.set_typing(&target, TypingState::Active));
        p.typing_sent.set(Some(Instant::now()));
    }
    if let Some(old) = p.typing_pause.take() {
        old.remove();
    }
    let weak = Rc::downgrade(p);
    let source = glib::timeout_add_local_once(Duration::from_secs(6), move || {
        let Some(p) = weak.upgrade() else {
            return;
        };
        p.typing_pause.replace(None);
        p.typing_sent.set(None);
        if let Some(c) = p.ctrl.upgrade() {
            c.act(move |s| s.set_typing(&id, TypingState::Paused));
        }
    });
    p.typing_pause.replace(Some(source));
}

fn stop_typing(p: &Inner) {
    if let Some(old) = p.typing_pause.take() {
        old.remove();
    }
    if p.typing_sent.take().is_some()
        && let (Some(id), Some(c)) = (p.buffer.borrow().clone(), p.ctrl.upgrade())
    {
        c.act(move |s| s.set_typing(&id, TypingState::Done));
    }
}

// MARK: Rows

fn label(markup: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder()
        .use_markup(true)
        .label(markup)
        .xalign(0.0)
        .valign(gtk::Align::BaselineFill)
        .build();
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn text_label(
    markup: &str,
    ctrl: &Weak<Controller>,
    pane: &Weak<Inner>,
    previews: bool,
) -> gtk::Label {
    let l = gtk::Label::builder()
        .use_markup(true)
        .label(markup)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .hexpand(true)
        .selectable(true)
        .valign(gtk::Align::BaselineFill)
        .css_classes(["chat-text"])
        .build();
    // Selectable labels take focus on click; keep it in the composer.
    l.set_focus_on_click(false);
    connect_links(&l, ctrl, pane, previews);
    l
}

/// Links look like text: Ctrl-hover underlines one, Ctrl-click opens it
/// (channels are joined). Resting on a web link shows its preview.
fn connect_links(l: &gtk::Label, ctrl: &Weak<Controller>, pane: &Weak<Inner>, previews: bool) {
    let ctrl_activate = ctrl.clone();
    let pane = pane.clone();
    l.connect_activate_link(move |label, uri| {
        let held = label
            .display()
            .default_seat()
            .and_then(|s| s.keyboard())
            .is_some_and(|k| k.modifier_state().contains(gdk::ModifierType::CONTROL_MASK));
        if !held {
            return glib::Propagation::Stop;
        }
        let Some(c) = ctrl_activate.upgrade() else {
            return glib::Propagation::Stop;
        };
        if let Some(channel) = channel_of(uri) {
            let network = pane
                .upgrade()
                .and_then(|p| p.buffer.borrow().as_ref().map(|b| b.network.clone()));
            if let Some(network) = network {
                let channel = channel.to_owned();
                c.act(move |s| {
                    s.join(&network, &channel, None);
                });
            }
        } else {
            let launcher = gtk::UriLauncher::new(uri);
            let window = label.root().and_downcast::<gtk::Window>();
            launcher.launch(window.as_ref(), gio::Cancellable::NONE, |r| {
                if let Err(e) = r {
                    tracing::warn!("couldn't open a link: {e}");
                }
            });
        }
        glib::Propagation::Stop
    });
    let motion = gtk::EventControllerMotion::new();
    let target = l.clone();
    motion.connect_motion(move |m, _, _| {
        if m.current_event_state()
            .contains(gdk::ModifierType::CONTROL_MASK)
        {
            target.add_css_class("ctrl");
        } else {
            target.remove_css_class("ctrl");
        }
    });
    let target = l.clone();
    motion.connect_leave(move |_| target.remove_css_class("ctrl"));
    l.add_controller(motion);
    if previews {
        crate::desktop::previews::attach(l, ctrl.clone());
    }
}

fn hbox(spacing: i32) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    b.set_baseline_position(gtk::BaselinePosition::Top);
    b
}

fn time_label(ctx: &RowCtx, time: SystemTime, fixed: bool) -> Option<gtk::Label> {
    if ctx.timestamps == TimestampStyle::Hidden {
        return None;
    }
    let l = label(&glib::markup_escape_text(&ctx.time(time)), &["time"]);
    if fixed {
        let chars = if ctx.timestamps == TimestampStyle::Seconds {
            8
        } else {
            5
        };
        l.set_width_chars(chars);
        l.set_xalign(1.0);
    }
    Some(l)
}

fn nick_column(ctx: &RowCtx, markup: &str) -> gtk::Label {
    let l = label(markup, &["chat-text"]);
    l.set_width_chars(ctx.nick_width as i32);
    l.set_max_width_chars(ctx.nick_width as i32);
    l.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    l.set_xalign(1.0);
    l
}

fn build_row(ctx: &RowCtx, row: &Row, ctrl: &Weak<Controller>, pane: &Weak<Inner>) -> gtk::Widget {
    match row {
        Row::Header(state) => {
            let text = match state {
                HeaderState::More => "Scroll up for older messages",
                HeaderState::Loading => "Loading…",
                HeaderState::Beginning => "Beginning of history",
                HeaderState::Cleared => "Cleared",
            };
            let l = gtk::Label::new(Some(text));
            l.add_css_class("history-header");
            l.upcast()
        }
        Row::Item(TimelineItem::Day(date)) => {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            b.add_css_class("day");
            let left = gtk::Separator::new(gtk::Orientation::Horizontal);
            left.set_hexpand(true);
            left.set_valign(gtk::Align::Center);
            let right = gtk::Separator::new(gtk::Orientation::Horizontal);
            right.set_hexpand(true);
            right.set_valign(gtk::Align::Center);
            b.append(&left);
            b.append(&gtk::Label::new(Some(
                &date.format("%A, %-d %B %Y").to_string(),
            )));
            b.append(&right);
            b.upcast()
        }
        Row::Item(TimelineItem::UnreadLine) => {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            b.add_css_class("unread-line");
            let line = gtk::Separator::new(gtk::Orientation::Horizontal);
            line.set_hexpand(true);
            line.set_valign(gtk::Align::Center);
            let badge = gtk::Label::new(Some("New"));
            badge.add_css_class("badge");
            b.append(&line);
            b.append(&badge);
            b.update_property(&[gtk::accessible::Property::Label("New messages below")]);
            b.upcast()
        }
        Row::Item(TimelineItem::Presence(run)) => {
            let b = hbox(8);
            b.add_css_class("message");
            if let Some(t) = time_label(ctx, run.last().map_or(SystemTime::now(), |m| m.time), true)
            {
                b.append(&t);
            }
            if ctx.layout == MessageLayout::Columns {
                let arrows = nick_column(ctx, "⇄");
                arrows.add_css_class("muted");
                b.append(&arrows);
            }
            let summary = label(
                &glib::markup_escape_text(&timeline::summary(run)),
                &["muted", "chat-text"],
            );
            summary.set_wrap(true);
            summary.set_hexpand(true);
            summary.set_selectable(true);
            summary.set_focus_on_click(false);
            b.append(&summary);
            b.upcast()
        }
        Row::Item(TimelineItem::Message { message, continued }) => {
            let w = message_row(ctx, message, *continued, ctrl, pane);
            if !message.own && ctx.mentions_message(message) {
                w.add_css_class("mention");
            }
            if ctx.flash.as_deref() == Some(stable_id(message).as_str()) {
                w.add_css_class("flash");
            }
            w.upcast()
        }
    }
}

impl RowCtx {
    fn mentions_message(&self, m: &ChatMessage) -> bool {
        match m.kind {
            MessageKind::Privmsg | MessageKind::Notice | MessageKind::Action => {
                self.mentions.matches(&m.text)
            }
            MessageKind::Invite => self.kind == BufferKind::Server,
            _ => false,
        }
    }
}

fn message_row(
    ctx: &RowCtx,
    m: &ChatMessage,
    continued: bool,
    ctrl: &Weak<Controller>,
    pane: &Weak<Inner>,
) -> gtk::Box {
    let palette = &ctx.palette;
    let plain = backroom_ffi::strip_formatting(m.text.clone());
    let nick_color = ctx.nick_color(m);
    let nick = |italic: bool| {
        render::run(
            &ctx.nick_label(&m.sender),
            nick_color.as_deref(),
            true,
            italic,
        )
    };
    let hides_nick = continued && !ctx.repeat_nick;
    let body = |italic: bool| render::message_markup(&m.text, ctx.formatting, palette, italic);
    let previews = ctx.link_previews;
    let event_text = || {
        let who = &m.sender;
        let extra = |s: &str| {
            if s.is_empty() {
                String::new()
            } else {
                format!(" ({s})")
            }
        };
        match m.kind {
            MessageKind::Join => format!("{who} joined"),
            MessageKind::Part => format!("{who} left{}", extra(&plain)),
            MessageKind::Quit => format!("{who} quit{}", extra(&plain)),
            MessageKind::Kick => format!("{who} kicked {plain}"),
            MessageKind::Topic => format!("{who} set the topic: {plain}"),
            MessageKind::Nick if m.own => format!("You are now known as {plain}"),
            MessageKind::Nick => format!("{who} is now known as {plain}"),
            MessageKind::Mode => format!("{who} set mode {plain}"),
            MessageKind::Invite if ctx.kind == BufferKind::Server => {
                format!("{who} invited you to {plain}")
            }
            MessageKind::Invite => format!("{who} invited {plain}"),
            _ => plain.clone(),
        }
    };
    let arrow = match m.kind {
        MessageKind::Join => "→ ",
        MessageKind::Part | MessageKind::Kick | MessageKind::Quit => "← ",
        MessageKind::Nick | MessageKind::Mode | MessageKind::Invite => "* ",
        _ => "",
    };
    let event_row = |b: &gtk::Box, with_arrow: bool| {
        let text = if with_arrow {
            format!("{arrow}{}", event_text())
        } else {
            event_text()
        };
        let l = label(&glib::markup_escape_text(&text), &["muted", "chat-text"]);
        l.set_wrap(true);
        l.set_hexpand(true);
        l.set_selectable(true);
        l.set_focus_on_click(false);
        b.append(&l);
        // We were invited: offer to join.
        if m.kind == MessageKind::Invite && ctx.kind == BufferKind::Server {
            let button = gtk::Button::with_label(&format!("Join {plain}"));
            button.add_css_class("flat");
            button.set_valign(gtk::Align::Center);
            let (ctrl, network, channel) = (ctrl.clone(), ctx.network.clone(), plain.clone());
            button.connect_clicked(move |_| {
                if let Some(c) = ctrl.upgrade() {
                    let (network, channel) = (network.clone(), channel.clone());
                    c.act(move |s| {
                        s.join(&network, &channel, None);
                    });
                }
            });
            b.append(&button);
        }
    };
    let error_markup = |prefix: &str| {
        format!(
            "{}{}",
            render::run(prefix, Some(&palette.error), true, false),
            render::run(&plain, Some(&palette.error), false, false)
        )
    };

    let row = hbox(8);
    row.add_css_class("message");
    match ctx.layout {
        MessageLayout::Columns => {
            if let Some(t) = time_label(ctx, m.time, true) {
                row.append(&t);
            }
            let cell = match m.kind {
                MessageKind::Privmsg if hides_nick => String::new(),
                MessageKind::Privmsg => nick(false),
                MessageKind::Notice => format!("-{}-", nick(false)),
                MessageKind::Join => render::run("→", Some(&palette.muted), false, false),
                MessageKind::Part | MessageKind::Kick | MessageKind::Quit => {
                    render::run("←", Some(&palette.muted), false, false)
                }
                MessageKind::Reply => render::run("–", Some(&palette.muted), false, false),
                MessageKind::Error => render::run("!", Some(&palette.error), true, false),
                _ => render::run("*", Some(&palette.muted), false, false),
            };
            row.append(&nick_column(ctx, &cell));
            match m.kind {
                MessageKind::Privmsg | MessageKind::Notice | MessageKind::Reply => {
                    row.append(&text_label(&body(false), ctrl, pane, previews));
                }
                MessageKind::Error => row.append(&text_label(&error_markup(""), ctrl, pane, false)),
                MessageKind::Action => {
                    let markup = format!("{} {}", nick(true), body(true));
                    row.append(&text_label(&markup, ctrl, pane, previews));
                }
                _ => event_row(&row, false),
            }
        }
        MessageLayout::Inline | MessageLayout::Stacked => {
            if ctx.layout == MessageLayout::Stacked && m.kind == MessageKind::Privmsg {
                let column = gtk::Box::new(gtk::Orientation::Vertical, 1);
                column.set_hexpand(true);
                if !continued {
                    row.add_css_class("stacked-head");
                    let head = hbox(6);
                    head.append(&label(&nick(false), &["chat-text"]));
                    if let Some(t) = time_label(ctx, m.time, false) {
                        head.append(&t);
                    }
                    column.append(&head);
                }
                column.append(&text_label(&body(false), ctrl, pane, previews));
                row.append(&column);
                return row;
            }
            if let Some(t) = time_label(ctx, m.time, true) {
                row.append(&t);
            }
            match m.kind {
                MessageKind::Privmsg if hides_nick => {
                    let l = text_label(&body(false), ctrl, pane, previews);
                    l.set_margin_start((ctx.nick_width as i32) * 4);
                    row.append(&l);
                }
                MessageKind::Privmsg => {
                    let markup = format!("{}  {}", nick(false), body(false));
                    row.append(&text_label(&markup, ctrl, pane, previews));
                }
                MessageKind::Notice => {
                    let markup = format!("-{}-  {}", nick(false), body(false));
                    row.append(&text_label(&markup, ctrl, pane, previews));
                }
                MessageKind::Action => {
                    let markup = format!(
                        "{}{} {}",
                        render::run("* ", None, false, true),
                        nick(true),
                        body(true)
                    );
                    row.append(&text_label(&markup, ctrl, pane, previews));
                }
                MessageKind::Reply => row.append(&text_label(&body(false), ctrl, pane, previews)),
                MessageKind::Error => {
                    row.append(&text_label(&error_markup("! "), ctrl, pane, false))
                }
                _ if is_event(m.kind) => event_row(&row, true),
                _ => row.append(&text_label(&body(false), ctrl, pane, previews)),
            }
        }
    }
    row
}
