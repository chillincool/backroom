//! Preferences: account (saved servers), appearance, messages,
//! highlights, notifications.

use std::rc::Rc;

use adw::prelude::*;
use backroom_app::app::AppState;
use backroom_app::prefs::{
    AppearanceMode, Density, MONOSPACE, MessageLayout, NickColoring, Preferences, PresenceStyle,
    TimestampStyle,
};
use backroom_app::servers::ServerSettings;
use gtk::{gio, glib};

use crate::desktop::controller::Controller;

/// Changes a preference and saves it.
fn set(ctrl: &Rc<Controller>, f: impl FnOnce(&mut Preferences) + 'static) {
    ctrl.act(move |s| {
        f(&mut s.prefs);
        s.preferences_changed();
        s.apply_text_logs();
    });
}

fn combo<T: Copy + PartialEq + 'static>(
    ctrl: &Rc<Controller>,
    title: &str,
    options: &[(T, &str)],
    current: T,
    apply: impl Fn(&mut Preferences, T) + 'static,
) -> adw::ComboRow {
    let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&labels))
        .selected(options.iter().position(|(v, _)| *v == current).unwrap_or(0) as u32)
        .build();
    let values: Vec<T> = options.iter().map(|(v, _)| *v).collect();
    let (c, apply) = (ctrl.weak(), Rc::new(apply));
    row.connect_selected_notify(move |r| {
        let Some(&value) = values.get(r.selected() as usize) else {
            return;
        };
        if let Some(c) = c.upgrade() {
            let apply = apply.clone();
            set(&c, move |p| apply(p, value));
        }
    });
    row
}

fn switch(
    ctrl: &Rc<Controller>,
    title: &str,
    subtitle: &str,
    current: bool,
    apply: impl Fn(&mut Preferences, bool) + 'static,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .active(current)
        .build();
    let (c, apply) = (ctrl.weak(), Rc::new(apply));
    row.connect_active_notify(move |r| {
        let on = r.is_active();
        if let Some(c) = c.upgrade() {
            let apply = apply.clone();
            set(&c, move |p| apply(p, on));
        }
    });
    row
}

/// A comma-separated list in one entry.
fn list_row(
    ctrl: &Rc<Controller>,
    title: &str,
    current: &[String],
    apply: impl Fn(&mut Preferences, Vec<String>) + 'static,
) -> adw::EntryRow {
    let row = adw::EntryRow::builder()
        .title(title)
        .text(current.join(", "))
        .show_apply_button(true)
        .build();
    let (c, apply) = (ctrl.weak(), Rc::new(apply));
    row.connect_apply(move |r| {
        let items: Vec<String> = r
            .text()
            .split([',', ' '])
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        if let Some(c) = c.upgrade() {
            let apply = apply.clone();
            set(&c, move |p| apply(p, items));
        }
    });
    row
}

pub fn show(ctrl: &Rc<Controller>, parent: &gtk::Widget, page: Option<&str>) {
    let Some((prefs, state_servers, current, standalone, text_logs_default)) = ctrl.read(|s| {
        (
            s.prefs.clone(),
            s.servers.clone(),
            s.settings().id,
            s.is_standalone(),
            s.paths().map(|p| p.default_text_logs()),
        )
    }) else {
        return;
    };
    let dialog = adw::PreferencesDialog::new();
    dialog.set_search_enabled(true);

    // Account
    let account = adw::PreferencesPage::builder()
        .title("Account")
        .icon_name("system-users-symbolic")
        .name("account")
        .build();
    let servers = adw::PreferencesGroup::builder()
        .title("Servers")
        .description("soju bouncers you log in to, and No bouncer. One is in use at a time; each keeps its own login in your keyring.")
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add soju Server…")
        .action_name("app.add-server")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    servers.set_header_suffix(Some(&add));
    for server in &state_servers.servers {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&server.display_name()))
            .subtitle(glib::markup_escape_text(&if server.is_standalone() {
                "Connect to networks directly".to_owned()
            } else {
                server.server_url.clone()
            }))
            .activatable(true)
            .build();
        if server.id == current {
            let in_use = gtk::Label::builder()
                .label("In use")
                .css_classes(["dim-label", "caption"])
                .build();
            row.add_suffix(&in_use);
        } else {
            let use_button = gtk::Button::builder()
                .label("Use")
                .valign(gtk::Align::Center)
                .build();
            let (c, id, d) = (ctrl.weak(), server.id, dialog.clone());
            use_button.connect_clicked(move |_| {
                if let Some(c) = c.upgrade() {
                    c.select_server(id);
                }
                d.close();
            });
            row.add_suffix(&use_button);
        }
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (c, s, d) = (ctrl.weak(), server.clone(), dialog.clone());
        row.connect_activated(move |_| {
            if let Some(c) = c.upgrade() {
                crate::desktop::dialogs::server_editor(&c, d.upcast_ref(), Some(s.clone()));
            }
        });
        servers.add(&row);
    }
    account.add(&servers);
    if cfg!(debug_assertions) {
        let dev = adw::PreferencesGroup::builder()
            .title("Development")
            .build();
        let row = adw::ActionRow::builder()
            .title("Add Test Servers")
            .subtitle("The e2e stack: alice (single sign-on) and dave (password)")
            .activatable(true)
            .build();
        let (c, d) = (ctrl.weak(), dialog.clone());
        row.connect_activated(move |_| {
            if let Some(c) = c.upgrade() {
                c.save_server(ServerSettings::e2e());
                c.save_server(ServerSettings::e2e_password());
            }
            d.close();
        });
        dev.add(&row);
        account.add(&dev);
    }
    let session = adw::PreferencesGroup::new();
    let log_out = adw::ActionRow::builder()
        .title(if standalone { "Disconnect" } else { "Log Out" })
        .activatable(true)
        .build();
    log_out.add_css_class("error");
    let (c, d) = (ctrl.weak(), dialog.clone());
    log_out.connect_activated(move |_| {
        if let Some(c) = c.upgrade() {
            c.log_out();
        }
        d.close();
    });
    session.add(&log_out);
    account.add(&session);
    dialog.add(&account);

    // Appearance
    let appearance = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .name("appearance")
        .build();
    let style = adw::PreferencesGroup::builder().title("Style").build();
    style.add(&combo(
        ctrl,
        "Appearance",
        &[
            (AppearanceMode::System, "Follow system"),
            (AppearanceMode::Light, "Light"),
            (AppearanceMode::Dark, "Dark"),
        ],
        prefs.appearance,
        |p, v| p.appearance = v,
    ));
    for dark in [false, true] {
        let themes = prefs.themes(dark);
        let names: Vec<String> = themes
            .iter()
            .map(|t| {
                if t.is_built_in() {
                    t.name.clone()
                } else {
                    format!("{} (imported)", t.name)
                }
            })
            .collect();
        let ids: Vec<String> = themes.iter().map(|t| t.id.clone()).collect();
        let current = prefs.theme(dark).id;
        let row = adw::ComboRow::builder()
            .title(if dark { "Dark theme" } else { "Light theme" })
            .model(&gtk::StringList::new(
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .selected(ids.iter().position(|i| *i == current).unwrap_or(0) as u32)
            .build();
        let c = ctrl.weak();
        row.connect_selected_notify(move |r| {
            let Some(id) = ids.get(r.selected() as usize).cloned() else {
                return;
            };
            if let Some(c) = c.upgrade() {
                set(&c, move |p| {
                    if dark {
                        p.dark_theme = id;
                    } else {
                        p.light_theme = id;
                    }
                });
            }
        });
        style.add(&row);
    }
    let import = adw::ActionRow::builder()
        .title("Import Themes…")
        .subtitle("Theme files (JSON) from Backroom on any platform")
        .activatable(true)
        .build();
    import.add_suffix(&gtk::Image::from_icon_name("document-open-symbolic"));
    let (c, d) = (ctrl.weak(), dialog.clone());
    import.connect_activated(move |_| {
        let Some(c) = c.upgrade() else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Themes"));
        filter.add_suffix("json");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title("Import Themes")
            .filters(&filters)
            .build();
        let window = d.root().and_downcast::<gtk::Window>();
        let (c2, d2) = (c.weak(), d.clone());
        chooser.open(window.as_ref(), gio::Cancellable::NONE, move |result| {
            let (Ok(file), Some(c)) = (result, c2.upgrade()) else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            let message = match std::fs::read(&path) {
                Err(e) => format!("Couldn't read the file: {e}"),
                Ok(bytes) => {
                    let mut prefs = c.read(|s| s.prefs.clone()).unwrap_or_default();
                    match prefs.import_themes(&bytes) {
                        Ok(n) => {
                            let themes = prefs.custom_themes.clone();
                            set(&c, move |p| p.custom_themes = themes);
                            format!(
                                "Imported {n} theme{}. Reopen Preferences to pick it.",
                                if n == 1 { "" } else { "s" }
                            )
                        }
                        Err(e) => format!("Not a theme file: {e}"),
                    }
                }
            };
            d2.add_toast(adw::Toast::new(&message));
        });
    });
    style.add(&import);
    if !prefs.custom_themes.is_empty() {
        let remove = adw::ActionRow::builder()
            .title("Remove Imported Themes")
            .activatable(true)
            .build();
        let (c, d) = (ctrl.weak(), dialog.clone());
        remove.connect_activated(move |_| {
            if let Some(c) = c.upgrade() {
                set(&c, |p| p.custom_themes.clear());
            }
            d.add_toast(adw::Toast::new("Imported themes removed."));
        });
        style.add(&remove);
    }
    appearance.add(&style);

    let text = adw::PreferencesGroup::builder().title("Text").build();
    let font_mode = adw::ComboRow::builder()
        .title("Font")
        .model(&gtk::StringList::new(&[
            "Interface font",
            "Monospace font",
            "Custom",
        ]))
        .selected(match prefs.font_family.as_str() {
            "" => 0,
            MONOSPACE => 1,
            _ => 2,
        })
        .build();
    let font_dialog = gtk::FontDialog::builder().title("Chat Font").build();
    let font_button = gtk::FontDialogButton::builder()
        .dialog(&font_dialog)
        .level(gtk::FontLevel::Family)
        .valign(gtk::Align::Center)
        .build();
    if !prefs.font_family.is_empty() && prefs.font_family != MONOSPACE {
        font_button.set_font_desc(&gtk::pango::FontDescription::from_string(
            &prefs.font_family,
        ));
    }
    let custom_font = adw::ActionRow::builder().title("Custom font").build();
    custom_font.add_suffix(&font_button);
    custom_font.set_visible(font_mode.selected() == 2);
    let (c, b, row) = (ctrl.weak(), font_button.clone(), custom_font.clone());
    font_mode.connect_selected_notify(move |m| {
        row.set_visible(m.selected() == 2);
        let family = match m.selected() {
            0 => String::new(),
            1 => MONOSPACE.to_owned(),
            _ => b
                .font_desc()
                .and_then(|d| d.family().map(|f| f.to_string()))
                .unwrap_or_else(|| "Sans".into()),
        };
        if let Some(c) = c.upgrade() {
            set(&c, move |p| p.font_family = family);
        }
    });
    let c = ctrl.weak();
    font_button.connect_font_desc_notify(move |b| {
        let Some(family) = b
            .font_desc()
            .and_then(|d| d.family().map(|f| f.to_string()))
        else {
            return;
        };
        if let Some(c) = c.upgrade() {
            set(&c, move |p| p.font_family = family);
        }
    });
    let size = adw::SpinRow::with_range(0.0, *backroom_app::prefs::FONT_SIZES.end(), 1.0);
    size.set_title("Font size");
    size.set_subtitle("Points; 0 follows the interface font");
    size.set_value(prefs.font_size);
    let c = ctrl.weak();
    size.connect_value_notify(move |r| {
        let mut v = r.value();
        if v > 0.0 && v < *backroom_app::prefs::FONT_SIZES.start() {
            v = *backroom_app::prefs::FONT_SIZES.start();
        }
        if let Some(c) = c.upgrade() {
            set(&c, move |p| p.font_size = v);
        }
    });
    text.add(&font_mode);
    text.add(&custom_font);
    text.add(&size);
    text.add(&combo(
        ctrl,
        "Density",
        &[
            (Density::Compact, "Compact"),
            (Density::Regular, "Regular"),
            (Density::Comfortable, "Comfortable"),
        ],
        prefs.density,
        |p, v| p.density = v,
    ));
    appearance.add(&text);
    dialog.add(&appearance);

    // Messages
    let messages = adw::PreferencesPage::builder()
        .title("Messages")
        .icon_name("mail-unread-symbolic")
        .name("messages")
        .build();
    let layout = adw::PreferencesGroup::builder().title("Layout").build();
    layout.add(&combo(
        ctrl,
        "Message layout",
        &[
            (MessageLayout::Columns, "Columns (time, nick, text)"),
            (MessageLayout::Inline, "Inline"),
            (MessageLayout::Stacked, "Stacked (nick above messages)"),
        ],
        prefs.layout,
        |p, v| p.layout = v,
    ));
    let width = adw::SpinRow::with_range(
        f64::from(*backroom_app::prefs::NICK_COLUMN_WIDTHS.start()),
        f64::from(*backroom_app::prefs::NICK_COLUMN_WIDTHS.end()),
        1.0,
    );
    width.set_title("Nick column width");
    width.set_subtitle("In characters");
    width.set_value(f64::from(prefs.nick_column_width));
    let c = ctrl.weak();
    width.connect_value_notify(move |r| {
        let v = r.value() as u32;
        if let Some(c) = c.upgrade() {
            set(&c, move |p| p.nick_column_width = v);
        }
    });
    layout.add(&width);
    layout.add(&switch(
        ctrl,
        "Repeat nicks",
        "Show the nick on every message, not only the first of a run",
        prefs.repeat_nick,
        |p, v| p.repeat_nick = v,
    ));
    layout.add(&combo(
        ctrl,
        "Timestamps",
        &[
            (TimestampStyle::Hidden, "Hidden"),
            (TimestampStyle::Minutes, "Hours and minutes"),
            (TimestampStyle::Seconds, "With seconds"),
        ],
        prefs.timestamps,
        |p, v| p.timestamps = v,
    ));
    layout.add(&combo(
        ctrl,
        "Joins and parts",
        &[
            (PresenceStyle::Show, "Show each"),
            (PresenceStyle::Fold, "Fold into one line"),
            (PresenceStyle::Hide, "Hide"),
        ],
        prefs.presence,
        |p, v| p.presence = v,
    ));
    messages.add(&layout);
    let colours = adw::PreferencesGroup::builder()
        .title("Colours and links")
        .build();
    colours.add(&combo(
        ctrl,
        "Nick colours",
        &[
            (NickColoring::Rank, "By rank (ops, voiced, …)"),
            (NickColoring::Nick, "By nick"),
            (NickColoring::None, "None"),
        ],
        prefs.nick_coloring,
        |p, v| p.nick_coloring = v,
    ));
    colours.add(&switch(
        ctrl,
        "Show formatting",
        "mIRC colours, bold and the like",
        prefs.show_formatting,
        |p, v| p.show_formatting = v,
    ));
    colours.add(&switch(
        ctrl,
        "Link previews",
        "Rest the mouse on a link to preview it. Only that link is fetched, which tells its site your address.",
        prefs.link_previews,
        |p, v| p.link_previews = v,
    ));
    messages.add(&colours);
    let typing = adw::PreferencesGroup::new();
    typing.add(&switch(
        ctrl,
        "Send typing notifications",
        "Let others see when you're typing",
        prefs.send_typing,
        |p, v| p.send_typing = v,
    ));
    messages.add(&typing);
    if standalone {
        let logs = adw::PreferencesGroup::builder()
            .title("Plain-text logs")
            .description("Without a bouncer, history is kept in a database on this computer. Backroom can also write one text file per conversation per day.")
            .build();
        let folder = prefs
            .text_log_dir(
                text_logs_default
                    .as_deref()
                    .unwrap_or(std::path::Path::new("")),
            )
            .display()
            .to_string();
        logs.add(&switch(
            ctrl,
            "Write text logs",
            &folder,
            prefs.text_logs,
            |p, v| p.text_logs = v,
        ));
        messages.add(&logs);
    }
    dialog.add(&messages);

    // Highlights
    let highlights = adw::PreferencesPage::builder()
        .title("Highlights")
        .icon_name("starred-symbolic")
        .name("highlights")
        .build();
    let words = adw::PreferencesGroup::builder()
        .title("Highlights")
        .description("Messages with your nick or one of these words count as mentions.")
        .build();
    words.add(&list_row(
        ctrl,
        "Words (comma-separated)",
        &prefs.highlight_words,
        |p, v| p.highlight_words = v,
    ));
    highlights.add(&words);
    let ignore = adw::PreferencesGroup::builder()
        .title("Ignored")
        .description(
            "Messages from these nicks are hidden. /ignore and /unignore change the list too.",
        )
        .build();
    ignore.add(&list_row(
        ctrl,
        "Nicks (comma-separated)",
        &prefs.ignored,
        |p, v| p.ignored = v,
    ));
    highlights.add(&ignore);
    dialog.add(&highlights);

    // Notifications
    let notifications = adw::PreferencesPage::builder()
        .title("Notifications")
        .icon_name("preferences-system-notifications-symbolic")
        .name("notifications")
        .build();
    let notify = adw::PreferencesGroup::new();
    notify.add(&switch(
        ctrl,
        "Notifications",
        "For mentions and private messages you're not looking at",
        prefs.notifications,
        |p, v| p.notifications = v,
    ));
    notify.add(&switch(
        ctrl,
        "Every channel message",
        "Also notify for channel messages that don't mention you",
        prefs.notify_all_messages,
        |p, v| p.notify_all_messages = v,
    ));
    notifications.add(&notify);
    let background = adw::PreferencesGroup::new();
    background.add(&switch(
        ctrl,
        "Run in the background",
        "Closing the window keeps Backroom connected; quit from the menu",
        prefs.run_in_background,
        |p, v| p.run_in_background = v,
    ));
    notifications.add(&background);
    dialog.add(&notifications);

    if let Some(page) = page {
        dialog.set_visible_page_name(page);
    }
    dialog.present(Some(parent));
}

/// Applies the light/dark choice.
pub fn apply_appearance(state: &AppState) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(match state.prefs.appearance {
        AppearanceMode::System => adw::ColorScheme::Default,
        AppearanceMode::Light => adw::ColorScheme::ForceLight,
        AppearanceMode::Dark => adw::ColorScheme::ForceDark,
    });
}
