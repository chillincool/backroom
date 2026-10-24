//! Sans-IO connection state machines. They consume parsed messages and
//! produce outgoing ones; the async driver in [`crate::client`] does the I/O.

pub mod registration;

pub use registration::{Outgoing, Registered, Registration, RegistrationConfig, RegistrationError};
