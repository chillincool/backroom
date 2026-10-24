//! Backroom core: a platform-independent IRC client library for the soju
//! bouncer. Front ends (CLI now, SwiftUI via UniFFI later) supply only
//! platform services: opening a browser for login and storing tokens.

pub mod auth;
pub mod batch;
pub mod bouncer;
pub mod client;
pub mod proto;
pub mod sasl;
pub mod session;
pub mod transport;

pub use client::{Client, ClientError, ConnectOptions};
