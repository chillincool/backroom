//! The login page: pick a saved server, then log in with a password, in
//! the browser (single sign-on), or connect without a bouncer.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use backroom_app::app::{AppState, Phase};
use backroom_app::servers::Login;
use gtk::glib;
use uuid::Uuid;

use crate::desktop::controller::Controller;

pub struct LoginPage {
    pub widget: adw::ToolbarView,
    status: adw::StatusPage,
    servers: gtk::DropDown,
    server_ids: std::cell::RefCell<Vec<Uuid>>,
    username: adw::ActionRow,
    password: adw::PasswordEntryRow,
    group: adw::PreferencesGroup,
    log_in: gtk::Button,
    browser: gtk::Button,
    connect: gtk::Button,
    cancel: gtk::Button,
    retry: gtk::Button,
    waiting: gtk::Box,
    notice: gtk::Label,
    keyring: gtk::Label,
    syncing: Cell<bool>,
}

impl LoginPage {
    pub fn new(ctrl: &Rc<Controller>) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        let menu = gtk::gio::Menu::new();
        menu.append(Some("Preferences"), Some("app.preferences"));
        menu.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        menu.append(Some("About Backroom"), Some("app.about"));
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .tooltip_text("Main Menu")
                .primary(true)
                .build(),
        );

        let servers = gtk::DropDown::from_strings(&[]);
        servers.set_hexpand(true);
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add soju Server…")
            .action_name("app.add-server")
            .css_classes(["flat"])
            .build();
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Edit Server…")
            .action_name("app.edit-server")
            .css_classes(["flat"])
            .build();
        let server_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        server_row.append(&servers);
        server_row.append(&edit);
        server_row.append(&add);

        let username = adw::ActionRow::builder().title("Username").build();
        username.add_css_class("property");
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        let group = adw::PreferencesGroup::new();
        group.add(&username);
        group.add(&password);

        let button = |label: &str| {
            gtk::Button::builder()
                .label(label)
                .halign(gtk::Align::Center)
                .css_classes(["pill", "suggested-action"])
                .build()
        };
        let log_in = button("Log In");
        let browser = button("Log In with Browser");
        let connect = button("Connect");
        let retry = button("Try Again");
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 12);
        waiting.append(
            &gtk::Spinner::builder()
                .spinning(true)
                .width_request(32)
                .height_request(32)
                .build(),
        );
        waiting.append(
            &gtk::Label::builder()
                .label("Finish logging in in your browser…")
                .css_classes(["dim-label"])
                .build(),
        );
        waiting.append(&cancel);
        let notice = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["error"])
            .visible(false)
            .build();
        let keyring = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .label(
                "No keyring service was found, so logins are only remembered until Backroom quits.",
            )
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.append(&server_row);
        content.append(&group);
        content.append(&notice);
        content.append(&log_in);
        content.append(&browser);
        content.append(&connect);
        content.append(&retry);
        content.append(&waiting);
        content.append(&keyring);
        let clamp = adw::Clamp::builder()
            .maximum_size(400)
            .child(&content)
            .build();
        let status = adw::StatusPage::builder()
            .icon_name(crate::desktop::APP_ID)
            .title("Backroom")
            .child(&clamp)
            .vexpand(true)
            .build();
        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.set_content(Some(&status));

        let page = Rc::new(LoginPage {
            widget,
            status,
            servers,
            server_ids: Default::default(),
            username,
            password,
            group,
            log_in,
            browser,
            connect,
            cancel,
            retry,
            waiting,
            notice,
            keyring,
            syncing: Cell::new(false),
        });
        page.connect(ctrl);
        page
    }

    fn connect(self: &Rc<Self>, ctrl: &Rc<Controller>) {
        let (weak, c) = (Rc::downgrade(self), ctrl.weak());
        self.servers.connect_selected_notify(move |dd| {
            let (Some(page), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                return;
            };
            if page.syncing.get() {
                return;
            }
            let id = page
                .server_ids
                .borrow()
                .get(dd.selected() as usize)
                .copied();
            if let Some(id) = id {
                c.select_server(id);
            }
        });
        let submit = {
            let (weak, c) = (Rc::downgrade(self), ctrl.weak());
            move || {
                let (Some(page), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                    return;
                };
                let password = page.password.text().to_string();
                page.password.set_text("");
                c.log_in_with_password(password);
            }
        };
        let s = submit.clone();
        self.log_in.connect_clicked(move |_| s());
        self.password.connect_entry_activated(move |_| submit());
        let c = ctrl.weak();
        self.browser.connect_clicked(move |_| {
            if let Some(c) = c.upgrade() {
                c.log_in_with_browser();
            }
        });
        let c = ctrl.weak();
        self.connect.connect_clicked(move |_| {
            if let Some(c) = c.upgrade() {
                c.connect_standalone();
            }
        });
        let c = ctrl.weak();
        self.cancel.connect_clicked(move |_| {
            if let Some(c) = c.upgrade() {
                c.cancel_login();
            }
        });
        let c = ctrl.weak();
        self.retry.connect_clicked(move |_| {
            if let Some(c) = c.upgrade() {
                c.start(true);
            }
        });
    }

    pub fn refresh(&self, state: &AppState) {
        self.syncing.set(true);
        let names: Vec<String> = state
            .servers
            .servers
            .iter()
            .map(|s| s.display_name())
            .collect();
        let ids: Vec<Uuid> = state.servers.servers.iter().map(|s| s.id).collect();
        if *self.server_ids.borrow() != ids
            || self
                .servers
                .model()
                .is_none_or(|m| m.n_items() as usize != names.len())
        {
            let list = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
            self.servers.set_model(Some(&list));
            self.server_ids.replace(ids.clone());
        } else if let Some(list) = self.servers.model().and_downcast::<gtk::StringList>() {
            for (i, name) in names.iter().enumerate() {
                if list.string(i as u32).as_deref() != Some(name.as_str()) {
                    list.splice(i as u32, 1, &[name.as_str()]);
                }
            }
        }
        let settings = state.settings();
        if let Some(i) = ids.iter().position(|id| *id == settings.id) {
            self.servers.set_selected(i as u32);
        }
        self.syncing.set(false);

        let phase = &state.phase;
        let logging_in = *phase == Phase::LoggingIn;
        let failed = matches!(phase, Phase::Failed(_));
        let login = settings.login;
        let complete = settings.is_complete();
        self.status.set_description(Some(&glib::markup_escape_text(&match login {
            Login::Password => format!("Log in to {} with your soju account.", settings.display_name()),
            Login::SingleSignOn => format!("Log in to {} in your browser.", settings.display_name()),
            Login::Standalone => {
                "Connect to IRC networks directly, without a bouncer. History is kept on this computer."
                    .to_owned()
            }
        })));
        self.username
            .set_subtitle(&glib::markup_escape_text(&settings.username));
        self.group
            .set_visible(login == Login::Password && complete && !failed);
        self.password.set_sensitive(!logging_in);
        self.log_in
            .set_visible(login == Login::Password && complete && !failed);
        self.log_in.set_sensitive(!logging_in);
        self.browser
            .set_visible(login == Login::SingleSignOn && !logging_in && !failed);
        self.connect
            .set_visible(login == Login::Standalone && !failed);
        self.waiting
            .set_visible(login == Login::SingleSignOn && logging_in);
        self.retry.set_visible(failed);
        let notice = match phase {
            Phase::Failed(reason) => Some(format!("Couldn't start: {reason}")),
            _ if !complete => {
                Some("This server's settings are incomplete. Edit it to finish.".into())
            }
            _ => state.notice.clone(),
        };
        self.notice.set_visible(notice.is_some());
        self.notice.set_text(notice.as_deref().unwrap_or_default());
        self.keyring
            .set_visible(crate::desktop::secrets::keyring_unavailable());
    }
}
