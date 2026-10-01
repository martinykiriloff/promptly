//! Per-session state machine. Fuses the three signals (PTY, hooks,
//! transcript) into one state; losing a signal degrades accuracy, never the
//! terminal.

use crate::hooks::{HookEvent, NotificationKind, is_test_command};
use crate::transcript::TranscriptActivity;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Starting,
    Working,
    WaitingPermission,
    WaitingInput,
    Idle,
    Done,
    Error,
    /// The PTY child exited cleanly.
    Exited,
}

impl SessionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::WaitingPermission => "waiting-permission",
            Self::WaitingInput => "waiting-input",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Error => "error",
            Self::Exited => "exited",
        }
    }

    /// States where the session is blocked on the human.
    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::WaitingPermission | Self::WaitingInput | Self::Done | Self::Error
        )
    }

    /// Higher is more urgent; used to rank the attention queue.
    pub fn urgency(self) -> u8 {
        match self {
            Self::WaitingPermission => 4,
            Self::WaitingInput => 3,
            Self::Error => 2,
            Self::Done => 1,
            _ => 0,
        }
    }
}

/// Which signal last drove the state, shown in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Pty,
    Hook,
    Transcript,
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub from: SessionState,
    pub to: SessionState,
    /// Human-readable reason, e.g. the pending tool call.
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct StateTracker {
    state: SessionState,
    since: Instant,
    turn_started: Option<Instant>,
    last_turn: Option<Duration>,
    files_changed_this_turn: bool,
    tests_this_turn: Option<bool>,
    pending_tool: Option<String>,
    hooks_seen: bool,
    pub last_signal: Option<Signal>,
}

impl Default for StateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StateTracker {
    pub fn new() -> Self {
        Self {
            state: SessionState::Starting,
            since: Instant::now(),
            turn_started: None,
            last_turn: None,
            files_changed_this_turn: false,
            tests_this_turn: None,
            pending_tool: None,
            hooks_seen: false,
            last_signal: None,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn since(&self) -> Instant {
        self.since
    }

    pub fn hooks_seen(&self) -> bool {
        self.hooks_seen
    }

    pub fn pending_tool(&self) -> Option<&str> {
        self.pending_tool.as_deref()
    }

    /// Elapsed time of the running turn, or the duration of the last one.
    pub fn turn_elapsed(&self) -> Option<Duration> {
        self.turn_started.map(|t| t.elapsed()).or(self.last_turn)
    }

    fn set(
        &mut self,
        to: SessionState,
        signal: Signal,
        detail: Option<String>,
    ) -> Option<Transition> {
        self.last_signal = Some(signal);
        if to == self.state {
            return None;
        }
        let from = self.state;
        self.state = to;
        self.since = Instant::now();
        Some(Transition { from, to, detail })
    }

    fn begin_turn(&mut self) {
        self.turn_started = Some(Instant::now());
        self.files_changed_this_turn = false;
        self.tests_this_turn = None;
    }

    fn end_turn(&mut self) {
        self.last_turn = self.turn_started.take().map(|t| t.elapsed());
    }

    pub fn on_hook(&mut self, ev: &HookEvent) -> Option<Transition> {
        self.hooks_seen = true;
        if matches!(self.state, SessionState::Exited | SessionState::Error) {
            return None;
        }
        match ev.hook_event_name.as_str() {
            "SessionStart" => self.set(SessionState::Idle, Signal::Hook, None),
            "UserPromptSubmit" => {
                self.begin_turn();
                self.set(SessionState::Working, Signal::Hook, None)
            }
            "PreToolUse" => {
                if self.turn_started.is_none() {
                    self.begin_turn();
                }
                self.pending_tool = ev.tool_summary();
                self.set(SessionState::Working, Signal::Hook, None)
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let ok = ev.hook_event_name == "PostToolUse";
                if ok && ev.is_file_change() {
                    self.files_changed_this_turn = true;
                }
                if let Some(cmd) = ev.bash_command()
                    && is_test_command(cmd)
                {
                    // Last test run in the turn wins.
                    self.tests_this_turn = Some(ok && !tool_reported_failure(ev));
                }
                self.pending_tool = None;
                self.set(SessionState::Working, Signal::Hook, None)
            }
            "PermissionRequest" => {
                let detail = ev.tool_summary().or_else(|| self.pending_tool.clone());
                self.set(SessionState::WaitingPermission, Signal::Hook, detail)
            }
            "Notification" => match ev.notification_kind() {
                NotificationKind::Permission => {
                    let detail = self.pending_tool.clone().or_else(|| ev.message.clone());
                    self.set(SessionState::WaitingPermission, Signal::Hook, detail)
                }
                // An idle reminder after a finished turn is not new information.
                NotificationKind::Idle if self.state != SessionState::Done => {
                    self.set(SessionState::WaitingInput, Signal::Hook, ev.message.clone())
                }
                _ => None,
            },
            "Stop" => {
                self.end_turn();
                self.pending_tool = None;
                let done = self.files_changed_this_turn && self.tests_this_turn == Some(true);
                let to = if done {
                    SessionState::Done
                } else {
                    SessionState::Idle
                };
                self.set(to, Signal::Hook, None)
            }
            _ => None,
        }
    }

    /// Channel 3 fallback: only used while no hook has ever arrived.
    pub fn on_transcript(&mut self, activity: TranscriptActivity) -> Option<Transition> {
        if self.hooks_seen || matches!(self.state, SessionState::Exited | SessionState::Error) {
            return None;
        }
        match activity {
            TranscriptActivity::Working => {
                if self.turn_started.is_none() {
                    self.begin_turn();
                }
                self.set(SessionState::Working, Signal::Transcript, None)
            }
            TranscriptActivity::TurnEnded => {
                self.end_turn();
                self.set(SessionState::Idle, Signal::Transcript, None)
            }
            TranscriptActivity::Unknown => None,
        }
    }

    /// Channel 1 fallback: a bell from a session with no hooks is the best
    /// "needs you" signal a plain terminal has.
    pub fn on_bell(&mut self) -> Option<Transition> {
        if self.hooks_seen || self.state != SessionState::Working {
            return None;
        }
        self.set(SessionState::WaitingInput, Signal::Pty, Some("Bell".into()))
    }

    pub fn on_exit(&mut self, code: Option<i32>) -> Option<Transition> {
        self.end_turn();
        match code {
            Some(0) => self.set(SessionState::Exited, Signal::Pty, None),
            Some(c) => self.set(
                SessionState::Error,
                Signal::Pty,
                Some(format!("exit code {c}")),
            ),
            None => self.set(
                SessionState::Error,
                Signal::Pty,
                Some("terminated by signal".into()),
            ),
        }
    }

    /// The user typed into or focused the session: acknowledge a finished
    /// turn so it leaves the attention queue.
    pub fn acknowledge(&mut self) -> Option<Transition> {
        match self.state {
            SessionState::Done => self.set(SessionState::Idle, Signal::Pty, None),
            _ => None,
        }
    }
}

fn tool_reported_failure(ev: &HookEvent) -> bool {
    let Some(r) = ev.tool_response.as_ref() else {
        return false;
    };
    r.get("is_error").and_then(|v| v.as_bool()) == Some(true)
        || r.get("exit_code")
            .and_then(|v| v.as_i64())
            .is_some_and(|c| c != 0)
        || r.get("interrupted").and_then(|v| v.as_bool()) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hook(v: serde_json::Value) -> HookEvent {
        HookEvent::parse(&v).unwrap()
    }

    #[test]
    fn prd_state_table() {
        let mut t = StateTracker::new();
        t.on_hook(&hook(json!({"hook_event_name":"SessionStart"})));
        assert_eq!(t.state(), SessionState::Idle);
        t.on_hook(&hook(
            json!({"hook_event_name":"UserPromptSubmit","prompt":"fix it"}),
        ));
        assert_eq!(t.state(), SessionState::Working);
        t.on_hook(&hook(json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf build"}})));
        let tr = t
            .on_hook(&hook(json!({"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"})))
            .unwrap();
        assert_eq!(tr.to, SessionState::WaitingPermission);
        assert_eq!(tr.detail.as_deref(), Some("Bash: rm -rf build"));
        t.on_hook(&hook(json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf build"}})));
        assert_eq!(t.state(), SessionState::Working);
        t.on_hook(&hook(json!({"hook_event_name":"Stop"})));
        assert_eq!(t.state(), SessionState::Idle, "no file changes => idle");
        t.on_hook(&hook(
            json!({"hook_event_name":"Notification","notification_type":"idle_prompt"}),
        ));
        assert_eq!(t.state(), SessionState::WaitingInput);
    }

    #[test]
    fn done_requires_file_changes_and_passing_tests() {
        let mut t = StateTracker::new();
        t.on_hook(&hook(json!({"hook_event_name":"UserPromptSubmit"})));
        t.on_hook(&hook(json!({"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"a.rs"}})));
        t.on_hook(&hook(json!({"hook_event_name":"PostToolUseFailure","tool_name":"Bash","tool_input":{"command":"cargo test"}})));
        t.on_hook(&hook(json!({"hook_event_name":"Stop"})));
        assert_eq!(t.state(), SessionState::Idle, "failing tests => idle");

        t.on_hook(&hook(json!({"hook_event_name":"UserPromptSubmit"})));
        t.on_hook(&hook(json!({"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":"b.rs"}})));
        t.on_hook(&hook(json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"ok"}})));
        t.on_hook(&hook(json!({"hook_event_name":"Stop"})));
        assert_eq!(t.state(), SessionState::Done);
        t.acknowledge();
        assert_eq!(t.state(), SessionState::Idle);
    }

    #[test]
    fn pty_exit_codes() {
        let mut t = StateTracker::new();
        assert_eq!(t.on_exit(Some(1)).unwrap().to, SessionState::Error);
        let mut t = StateTracker::new();
        assert_eq!(t.on_exit(Some(0)).unwrap().to, SessionState::Exited);
        assert!(
            t.on_hook(&hook(json!({"hook_event_name":"UserPromptSubmit"})))
                .is_none()
        );
    }

    #[test]
    fn transcript_fallback_only_without_hooks() {
        let mut t = StateTracker::new();
        assert!(t.on_transcript(TranscriptActivity::Working).is_some());
        t.on_hook(&hook(json!({"hook_event_name":"Stop"})));
        assert!(t.on_transcript(TranscriptActivity::Working).is_none());
    }
}
