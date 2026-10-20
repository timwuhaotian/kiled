//! The language server client: a server's process and messages, the subset of LSP Kiled
//! speaks, and finding servers on disk (DESIGN.md §6).

mod client;
mod env;
pub mod protocol;
mod transport;

pub use client::{Client, Event};
pub use env::{find_program, find_server, login_path, server_path};
