//! Backroom for Linux (and the BSDs): a GTK 4 and libadwaita front end over
//! `backroom-app` and the core. Runs on any desktop (GNOME, KDE, Xfce, …);
//! platform services are the freedesktop ones: the Secret Service,
//! notifications, `x-scheme-handler/backroom` for the login callback.

#[cfg(all(unix, not(target_vendor = "apple")))]
mod desktop;

#[cfg(all(unix, not(target_vendor = "apple")))]
fn main() -> gtk::glib::ExitCode {
    desktop::main()
}

#[cfg(not(all(unix, not(target_vendor = "apple"))))]
fn main() {
    eprintln!("This is Backroom's GTK app for Linux; on macOS use the app in apple/.");
}
