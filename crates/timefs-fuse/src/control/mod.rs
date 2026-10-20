//! Control plane: a Unix socket through which the CLI talks to a running mount (ADR-0007).
//!
//! Protocol: one request line, then the server replies with text and closes the connection.
//! The first reply word is `ok` or `err`.

mod client;
mod protocol;
mod server;

pub use client::request;
pub use protocol::Command;
pub use server::ControlServer;
