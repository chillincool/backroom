//! UniFFI bindings for Backroom's core, used by the Swift front end.
//!
//! One [`BackroomClient`] per account: log in (`begin_login` /
//! `complete_login`), `connect`, then pull events with `next_event` and send
//! commands. Swift supplies a [`SecretStore`] (the Keychain) for the tokens;
//! they never leave Rust in any other form.

mod client;
mod error;
mod store;
mod types;

pub use client::{BackroomClient, ClientConfig};
pub use error::BackroomError;
pub use store::SecretStore;
pub use types::{
    BanEntry, BufferKind, ChannelListing, ChatMessage, ConnectionState, FormattedSpan, Member,
    MessageKind, Network, SearchHit, SessionEvent, TypingState, WhoisInfo,
};

uniffi::setup_scaffolding!();

/// Splits message text into runs of one mIRC style (bold, colours, …).
#[uniffi::export]
pub fn format_spans(text: String) -> Vec<FormattedSpan> {
    backroom_core::proto::formatting::parse_formatting(&text)
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Removes mIRC formatting codes (bold, colours, …) from message text.
#[uniffi::export]
pub fn strip_formatting(text: String) -> String {
    backroom_core::proto::formatting::strip_formatting(&text)
}
