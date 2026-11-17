//! The member list of the focused channel, with a moderation menu.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use backroom_app::app::AppState;
use backroom_app::buffer::BufferId;
use backroom_app::composer::ban_mask;
use backroom_app::prefs::NickColoring;
use backroom_ffi::{BufferKind, Member};
use gtk::{gdk, gio, glib};

use crate::desktop::controller::Controller;
use crate::desktop::style::Palette;

pub struct MemberList {
    pub widget: gtk::Box,
    title: gtk::Label,
    store: gio::ListStore,
    shown: RefCell<(Option<BufferId>, Vec<Member>)>,
    coloring: RefCell<(NickColoring, Option<Palette>)>,
}

impl MemberList {
    pub fn new(ctrl: &Rc<Controller>) -> Rc<Self> {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .single_click_activate(false)
            .css_classes(["member-list"])
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(12)
            .margin_top(8)
            .margin_bottom(4)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("member-list");
        widget.set_width_request(120);
        widget.append(&title);
        widget.append(&scroller);
        let members = Rc::new(MemberList {
            widget,
            title,
            store,
            shown: RefCell::default(),
            coloring: RefCell::new((NickColoring::Rank, None)),
        });
        members.setup(ctrl, &factory, &list);
        members
    }

    fn setup(
        self: &Rc<Self>,
        ctrl: &Rc<Controller>,
        factory: &gtk::SignalListItemFactory,
        list: &gtk::ListView,
    ) {
        let weak = Rc::downgrade(self);
        let c = ctrl.weak();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let label = gtk::Label::builder()
                .xalign(0.0)
                .use_markup(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .margin_start(12)
                .margin_end(6)
                .build();
            item.set_child(Some(&label));
            // Right-click: the member's menu. Double-click: a conversation.
            let click = gtk::GestureClick::builder().button(0).build();
            let (item_weak, members, c) = (item.downgrade(), weak.clone(), c.clone());
            click.connect_pressed(move |gesture, n, x, y| {
                let (Some(item), Some(members), Some(c)) =
                    (item_weak.upgrade(), members.upgrade(), c.upgrade())
                else {
                    return;
                };
                let Some(member) = item
                    .item()
                    .and_downcast::<glib::BoxedAnyObject>()
                    .map(|o| o.borrow::<Member>().clone())
                else {
                    return;
                };
                let button = gesture.current_button();
                if button == gdk::BUTTON_SECONDARY {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(widget) = gesture.widget() {
                        members.menu(&c, &widget, &member, x, y);
                    }
                } else if button == gdk::BUTTON_PRIMARY && n == 2 {
                    let buffer = members.shown.borrow().0.clone();
                    if let Some(buffer) = buffer {
                        c.act(move |s| s.open_query(&buffer.network, &member.nick));
                    }
                }
            });
            label.add_controller(click);
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let (Some(obj), Some(label), Some(members)) = (
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                item.child().and_downcast::<gtk::Label>(),
                weak.upgrade(),
            ) else {
                return;
            };
            let member = obj.borrow::<Member>();
            let (coloring, palette) = &*members.coloring.borrow();
            let prefix = member.prefix.chars().next();
            let color = palette.as_ref().and_then(|p| match coloring {
                NickColoring::Rank => p.rank(prefix).map(str::to_owned),
                NickColoring::Nick => Some(p.nick(&member.nick).to_owned()),
                NickColoring::None => p.text.clone(),
            });
            let text = format!(
                "{}{}",
                prefix.map(String::from).unwrap_or_default(),
                member.nick
            );
            let mut attrs = Vec::new();
            if let Some(c) = color {
                attrs.push(format!("foreground=\"{c}\""));
            }
            if member.away {
                attrs.push("alpha=\"50%\"".into());
            }
            label.set_markup(&format!(
                "<span {}>{}</span>",
                attrs.join(" "),
                glib::markup_escape_text(&text)
            ));
            label.set_tooltip_text(Some(&if member.away {
                format!("{} (away)", member.nick)
            } else {
                member.nick.clone()
            }));
        });
        let _ = list;
    }

    fn menu(&self, ctrl: &Rc<Controller>, widget: &gtk::Widget, member: &Member, x: f64, y: f64) {
        let Some(buffer) = self.shown.borrow().0.clone() else {
            return;
        };
        let channel = ctrl
            .read(|s| s.buffers.get(&buffer).map(|b| b.name.clone()))
            .flatten()
            .unwrap_or_default();
        let nick = member.nick.clone();
        let group = gio::SimpleActionGroup::new();
        let add = |name: &str, f: Box<dyn Fn(&mut AppState)>| {
            let action = gio::SimpleAction::new(name, None);
            let c = ctrl.weak();
            let f = Rc::new(f);
            action.connect_activate(move |_, _| {
                if let Some(c) = c.upgrade() {
                    let f = f.clone();
                    c.act(move |s| f(s));
                }
            });
            group.add_action(&action);
        };
        let (n, net, ch) = (nick.clone(), buffer.network.clone(), channel.clone());
        add("message", Box::new(move |s| s.open_query(&net, &n)));
        let (n, net) = (nick.clone(), buffer.network.clone());
        add(
            "whois",
            Box::new(move |s| {
                s.send_raw(&net, &format!("WHOIS {n}"));
            }),
        );
        for (name, mode) in [
            ("op", "+o"),
            ("deop", "-o"),
            ("voice", "+v"),
            ("devoice", "-v"),
        ] {
            let (n, net, ch) = (nick.clone(), buffer.network.clone(), ch.clone());
            add(
                name,
                Box::new(move |s| {
                    s.send_raw(&net, &format!("MODE {ch} {mode} {n}"));
                }),
            );
        }
        let (n, net, c2) = (nick.clone(), buffer.network.clone(), channel.clone());
        add(
            "kick",
            Box::new(move |s| {
                s.send_raw(&net, &format!("KICK {c2} {n}"));
            }),
        );
        let (n, net, c2) = (nick.clone(), buffer.network.clone(), channel.clone());
        add(
            "ban",
            Box::new(move |s| {
                s.send_raw(&net, &format!("MODE {c2} +b {}", ban_mask(&n)));
                s.send_raw(&net, &format!("KICK {c2} {n}"));
            }),
        );
        let n = nick.clone();
        add(
            "ignore",
            Box::new(move |s| {
                s.prefs.ignore(&n);
                s.preferences_changed();
            }),
        );
        widget.insert_action_group("member", Some(&group));

        let menu = gio::Menu::new();
        let top = gio::Menu::new();
        top.append(Some(&format!("Message {nick}")), Some("member.message"));
        top.append(Some("Info"), Some("member.whois"));
        menu.append_section(None, &top);
        let modes = gio::Menu::new();
        let has_op = member.prefix.contains('@');
        let has_voice = member.prefix.contains('+');
        modes.append(
            Some(if has_op {
                "Remove Operator"
            } else {
                "Make Operator"
            }),
            Some(if has_op { "member.deop" } else { "member.op" }),
        );
        modes.append(
            Some(if has_voice {
                "Remove Voice"
            } else {
                "Give Voice"
            }),
            Some(if has_voice {
                "member.devoice"
            } else {
                "member.voice"
            }),
        );
        modes.append(Some("Kick"), Some("member.kick"));
        modes.append(Some("Ban and Kick"), Some("member.ban"));
        menu.append_section(Some("Moderation"), &modes);
        let ignore = gio::Menu::new();
        ignore.append(Some(&format!("Ignore {nick}")), Some("member.ignore"));
        menu.append_section(None, &ignore);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(widget);
        popover.set_has_arrow(false);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let p = p.clone();
            // After the chosen action has run.
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    /// Shows the focused channel's members; hidden elsewhere.
    pub fn refresh(&self, state: &AppState, palette: &Palette, restyle: bool) -> bool {
        let selection = state.selection();
        let model = selection
            .as_ref()
            .and_then(|id| state.buffers.get(id))
            .filter(|b| b.kind == BufferKind::Channel);
        let Some(model) = model else {
            self.shown.replace((None, Vec::new()));
            self.store.remove_all();
            return false;
        };
        if restyle || self.coloring.borrow().1.as_ref() != Some(palette) {
            self.coloring
                .replace((state.prefs.nick_coloring, Some(palette.clone())));
            let n = self.store.n_items();
            self.store.items_changed(0, n, n);
        }
        let mut shown = self.shown.borrow_mut();
        if shown.0.as_ref() == Some(&model.id) && shown.1 == model.members() {
            return true;
        }
        *shown = (Some(model.id.clone()), model.members().to_vec());
        let objects: Vec<glib::BoxedAnyObject> = model
            .members()
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.store.splice(0, self.store.n_items(), &objects);
        let n = objects.len();
        let away = model.members().iter().filter(|m| m.away).count();
        self.title.set_text(&if away > 0 {
            format!("{n} members, {away} away")
        } else if n == 1 {
            "1 member".into()
        } else {
            format!("{n} members")
        });
        true
    }
}
