//! Promptly core: everything that is not pixels. The UI crate renders what
//! this crate derives; `promptly-ctl` uses its IPC client.

pub mod accounts;
pub mod attention;
pub mod commands;
pub mod config;
pub mod git;
pub mod hooks;
pub mod index;
pub mod ipc;
pub mod nl_command;
pub mod paths;
pub mod sanitize;
pub mod session_changes;
pub mod state;
pub mod statusline;
pub mod transcript;
pub mod update;
pub mod usage;
pub mod util;

/// Environment variables Promptly sets inside every pane.
pub mod env {
    pub const SOCKET: &str = "PROMPTLY_SOCKET";
    pub const TOKEN: &str = "PROMPTLY_TOKEN";
    pub const SESSION: &str = "PROMPTLY_SESSION";
    pub const USER_STATUSLINE: &str = "PROMPTLY_USER_STATUSLINE";
    pub const CONTROL: &str = "PROMPTLY_CONTROL";
}
