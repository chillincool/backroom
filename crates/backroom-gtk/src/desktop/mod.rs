//! Backroom for Linux: a GTK 4 and libadwaita front end over
//! `backroom-app` and the core. Runs on any desktop (GNOME, KDE, Xfce, …);
//! platform services are the freedesktop ones: the Secret Service,
//! notifications, `x-scheme-handler/backroom` for the login callback.

pub mod controller;
pub mod dialogs;
pub mod login;
pub mod members;
pub mod messages;
pub mod notify;
pub mod panes;
pub mod previews;
pub mod render;
pub mod secrets;
pub mod settings;
pub mod sidebar;
pub mod style;
pub mod window;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use backroom_app::storage::Paths;
use gtk::{gio, glib};

use self::controller::Controller;

pub const APP_ID: &str = "net.chillincool.backroom";

fn paths() -> Paths {
    let home = glib::home_dir();
    let base = |dir: std::path::PathBuf| dir.join("backroom");
    Paths {
        config: base(glib::user_config_dir()),
        data: base(glib::user_data_dir()),
        state: base(
            std::env::var_os("XDG_STATE_HOME")
                .map(Into::into)
                .unwrap_or_else(|| home.join(".local/state")),
        ),
    }
}

pub fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BACKROOM_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let controller: Rc<RefCell<Option<Rc<Controller>>>> = Rc::default();

    app.connect_startup(glib::clone!(
        #[strong]
        controller,
        move |app| {
            gtk::Window::set_default_icon_name(APP_ID);
            // Running from source: the icon isn't installed yet.
            #[cfg(debug_assertions)]
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::IconTheme::for_display(&display)
                    .add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons"));
            }
            let c = Controller::new(app, paths());
            window::install_actions(app, &c);
            c.start(true);
            *controller.borrow_mut() = Some(c);
        }
    ));
    app.connect_activate(glib::clone!(
        #[strong]
        controller,
        move |_| {
            if let Some(c) = controller.borrow().as_ref() {
                c.window().present();
            }
        }
    ));
    app.connect_open(glib::clone!(
        #[strong]
        controller,
        move |_, files, _| {
            let Some(c) = controller.borrow().clone() else {
                return;
            };
            c.window().present();
            for file in files {
                c.handle_uri(file.uri().to_string());
            }
        }
    ));
    app.connect_shutdown(glib::clone!(
        #[strong]
        controller,
        move |_| {
            if let Some(c) = controller.borrow_mut().take() {
                c.window().save_geometry();
                c.shutdown();
            }
        }
    ));
    app.run()
}
