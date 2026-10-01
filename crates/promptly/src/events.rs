use crate::pty::{PaneId, ShellMark};
use alacritty_terminal::event::Event as TermEvent;
use promptly_core::ipc::{ControlRequest, Responder};

/// Everything that reaches the UI thread from elsewhere.
pub enum AppEvent {
    Term(PaneId, TermEvent),
    Shell(PaneId, ShellMark),
    Control(ControlRequest, Responder),
    /// A notification was clicked: deep-link into the session (never approve).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    FocusSession(PaneId),
    IndexRefreshed,
    /// Press Enter in a pane (deferred after a composer paste).
    Submit(PaneId),
}
