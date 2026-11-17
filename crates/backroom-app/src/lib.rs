//! Backroom's app logic, shared by native front ends: everything between
//! the core's session events and the widgets on screen, with no toolkit
//! code. The GTK app uses it directly.
//!
//! - [`app::AppState`] applies session events and user actions and says
//!   what changed ([`app::Changes`]).
//! - [`buffer`], [`timeline`], [`composer`], [`mentions`], [`links`]: what
//!   a conversation shows and what typing into it does.
//! - [`panes`]: tabs and split panes. [`servers`], [`prefs`], [`theme`]:
//!   settings, saved as JSON under [`storage::Paths`].

pub mod app;
pub mod buffer;
pub mod composer;
pub mod links;
pub mod mentions;
pub mod panes;
pub mod prefs;
pub mod servers;
pub mod storage;
pub mod theme;
pub mod timeline;

pub use backroom_ffi as ffi;
