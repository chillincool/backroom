//! Desktop notifications (GNotification: the notification portal in a
//! Flatpak, else the desktop's notification service) and the unread badge
//! on launchers that show one (KDE, Ubuntu's dock, Dash to Dock).

use adw::prelude::*;
use backroom_app::app::AppState;
use backroom_app::buffer::BufferId;
use backroom_ffi::{BufferKind, ChatMessage, MessageKind};
use gtk::{gio, glib};

use crate::desktop::APP_ID;

pub fn post(app: &adw::Application, state: &AppState, id: &BufferId, message: &ChatMessage) {
    let Some(buffer) = state.buffers.get(id) else {
        return;
    };
    let text = backroom_ffi::strip_formatting(message.text.clone());
    let action = message.kind == MessageKind::Action;
    let (title, body) = match buffer.kind {
        BufferKind::Query => (
            message.sender.clone(),
            if action {
                format!("* {} {text}", message.sender)
            } else {
                text
            },
        ),
        _ => (
            format!("{} ({})", buffer.name, state.network_name(&id.network)),
            if action {
                format!("* {} {text}", message.sender)
            } else {
                format!("{}: {text}", message.sender)
            },
        ),
    };
    let notification = gio::Notification::new(&title);
    notification.set_body(Some(&body));
    if buffer.is_mention(message) || buffer.kind == BufferKind::Query {
        notification.set_priority(gio::NotificationPriority::High);
    }
    let target = (id.network.clone(), buffer.name.clone()).to_variant();
    notification.set_default_action_and_target_value("app.open-buffer", Some(&target));
    // One notification per conversation: a new message replaces the last.
    let tag = format!("{}|{}", id.network, id.key);
    app.send_notification(Some(&tag), &notification);
}

/// Withdraws a conversation's notification once it's read.
pub fn withdraw(app: &adw::Application, id: &BufferId) {
    app.withdraw_notification(&format!("{}|{}", id.network, id.key));
}

/// The unread count on the launcher icon (`com.canonical.Unity.LauncherEntry`).
pub fn badge(app: &adw::Application, count: usize) {
    thread_local! {
        static LAST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    if LAST.with(|l| l.replace(Some(count))) == Some(count) {
        return;
    }
    let Some(connection) = app.dbus_connection() else {
        return;
    };
    let props = glib::VariantDict::new(None);
    props.insert_value("count", &(count as i64).to_variant());
    props.insert_value("count-visible", &(count > 0).to_variant());
    let params = (format!("application://{APP_ID}.desktop"), props.end()).to_variant();
    let path = format!("/{}", APP_ID.replace('.', "/"));
    if let Err(e) = connection.emit_signal(
        None,
        &path,
        "com.canonical.Unity.LauncherEntry",
        "Update",
        Some(&params),
    ) {
        tracing::debug!("launcher badge: {e}");
    }
}
