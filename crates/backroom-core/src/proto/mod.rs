//! Wire-level IRC protocol: messages, capabilities, numerics, formatting.

pub mod caps;
pub mod formatting;
pub mod message;
pub mod numerics;

pub use message::{Message, ParseError};
