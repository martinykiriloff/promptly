//! Channel 2: the hooks sideband.
//!
//! Promptly never edits `~/.claude/settings.json`. Each Claude session is
//! launched with `--settings <file>` pointing at a session-scoped file that
//! only *adds* hooks; Claude Code merges hooks across sources, so the user's
//! own hooks keep running. The hook command is `promptly-ctl hook`, which
//! forwards the hook's stdin JSON to the session socket and exits.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Hook events Promptly subscribes to. Tool events take a `*` matcher.
pub const HOOK_EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("PostToolUseFailure", true),
    ("PermissionRequest", true),
    ("Notification", false),
    ("Stop", false),
    ("SubagentStop", false),
    ("SessionEnd", false),
];

/// Hard ceiling Claude Code applies to our hook command. The command itself
/// returns in well under 20 ms; this only bounds a pathological stall.
const HOOK_TIMEOUT_SECS: u32 = 2;

/// A hook payload, parsed defensively: every field is optional because the
/// schema changes between Claude Code releases.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookEvent {
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub tool_response: Option<Value>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

impl HookEvent {
    pub fn parse(v: &Value) -> Option<Self> {
        let ev: HookEvent = serde_json::from_value(v.clone()).ok()?;
        (!ev.hook_event_name.is_empty()).then_some(ev)
    }

    /// Classify a `Notification` hook. Newer CLIs send `notification_type`;
    /// older ones only send a human-readable message.
    pub fn notification_kind(&self) -> NotificationKind {
        match self.notification_type.as_deref() {
            Some("permission_prompt") => return NotificationKind::Permission,
            Some("idle_prompt") | Some("elicitation_dialog") => return NotificationKind::Idle,
            Some(_) => return NotificationKind::Other,
            None => {}
        }
        let m = self.message.as_deref().unwrap_or("").to_ascii_lowercase();
        if m.contains("permission") {
            NotificationKind::Permission
        } else if m.contains("waiting for your input") || m.contains("waiting for input") {
            NotificationKind::Idle
        } else {
            NotificationKind::Other
        }
    }

    pub fn is_file_change(&self) -> bool {
        matches!(
            self.tool_name.as_deref(),
            Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
        )
    }

    /// The Bash command, if this is a Bash tool event.
    pub fn bash_command(&self) -> Option<&str> {
        if self.tool_name.as_deref() != Some("Bash") {
            return None;
        }
        self.tool_input.as_ref()?.get("command")?.as_str()
    }

    /// One line describing the pending tool call, for the attention queue.
    pub fn tool_summary(&self) -> Option<String> {
        let name = self.tool_name.as_deref()?;
        let input = self.tool_input.as_ref();
        let detail = input.and_then(|i| {
            ["command", "file_path", "path", "url", "pattern"]
                .iter()
                .find_map(|k| i.get(*k).and_then(Value::as_str))
        });
        Some(match detail {
            Some(d) => format!("{name}: {}", crate::sanitize::one_line(d, 120)),
            None => name.to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Permission,
    Idle,
    Other,
}

/// Heuristic: does this shell command run a test suite?
pub fn is_test_command(cmd: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "cargo test",
        "cargo nextest",
        "npm test",
        "npm run test",
        "pnpm test",
        "pnpm run test",
        "yarn test",
        "bun test",
        "pytest",
        "python -m pytest",
        "go test",
        "jest",
        "vitest",
        "mix test",
        "rspec",
        "make test",
        "gradle test",
        "./gradlew test",
        "mvn test",
        "dotnet test",
        "swift test",
        "ctest",
        "phpunit",
    ];
    let c = cmd.trim_start();
    PATTERNS.iter().any(|p| c.contains(p))
}

/// The session-scoped settings Promptly passes via `--settings`.
#[derive(Debug, Clone)]
pub struct InjectedSettings {
    pub json: Value,
}

impl InjectedSettings {
    /// `ctl` is the absolute path to the `promptly-ctl` binary.
    /// `wrap_statusline` installs a status line that forwards cost/context
    /// to Promptly and then runs the user's own status line command, if any.
    pub fn build(ctl: &Path, wrap_statusline: bool) -> Self {
        let cmd = |sub: &str| format!("{} {sub}", shell_quote(&ctl.to_string_lossy()));
        let mut hooks = serde_json::Map::new();
        for (event, tool) in HOOK_EVENTS {
            let mut entry = json!({
                "hooks": [{ "type": "command", "command": cmd("hook"), "timeout": HOOK_TIMEOUT_SECS }]
            });
            if *tool {
                entry["matcher"] = json!("*");
            }
            hooks.insert((*event).to_string(), json!([entry]));
        }
        let mut json = json!({ "hooks": hooks });
        if wrap_statusline {
            json["statusLine"] =
                json!({ "type": "command", "command": cmd("statusline"), "padding": 0 });
        }
        Self { json }
    }

    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        let body = serde_json::to_vec_pretty(&self.json).expect("settings serialize");
        crate::util::write_private(path, &body)
    }
}

/// The user's own status line command, so the wrapper can chain to it.
/// Precedence mirrors Claude Code: project-local, project, user.
pub fn user_statusline_command(cwd: &Path) -> Option<String> {
    let candidates = [
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
        crate::paths::claude_dir().join("settings.json"),
    ];
    candidates.iter().find_map(|p| {
        let v: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
        let sl = v.get("statusLine")?;
        (sl.get("type").and_then(Value::as_str) == Some("command"))
            .then(|| sl.get("command")?.as_str().map(str::to_owned))
            .flatten()
    })
}

/// True when organization-managed settings disable hooks, in which case we
/// say so in the UI and rely on transcript tailing.
pub fn managed_hooks_disabled() -> bool {
    let paths: &[&str] = if cfg!(target_os = "macos") {
        &["/Library/Application Support/ClaudeCode/managed-settings.json"]
    } else {
        &["/etc/claude-code/managed-settings.json"]
    };
    paths.iter().any(|p| {
        std::fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| {
                v.get("disableAllHooks").and_then(Value::as_bool) == Some(true)
                    || v.get("allowManagedHooksOnly").and_then(Value::as_bool) == Some(true)
            })
            .unwrap_or(false)
    })
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_settings_cover_every_event_and_quote_paths() {
        let s = InjectedSettings::build(Path::new("/Applications/My App/promptly-ctl"), true);
        for (e, tool) in HOOK_EVENTS {
            let entry = &s.json["hooks"][e][0];
            assert_eq!(
                entry["hooks"][0]["command"],
                "'/Applications/My App/promptly-ctl' hook"
            );
            assert_eq!(entry.get("matcher").is_some(), *tool);
        }
        assert_eq!(
            s.json["statusLine"]["command"],
            "'/Applications/My App/promptly-ctl' statusline"
        );
    }

    #[test]
    fn notification_classification_old_and_new_cli() {
        let new = HookEvent {
            notification_type: Some("permission_prompt".into()),
            ..Default::default()
        };
        assert_eq!(new.notification_kind(), NotificationKind::Permission);
        let old = HookEvent {
            message: Some("Claude needs your permission to use Bash".into()),
            ..Default::default()
        };
        assert_eq!(old.notification_kind(), NotificationKind::Permission);
        let idle = HookEvent {
            message: Some("Claude is waiting for your input".into()),
            ..Default::default()
        };
        assert_eq!(idle.notification_kind(), NotificationKind::Idle);
    }

    #[test]
    fn parse_tolerates_unknown_and_missing_fields() {
        let v = json!({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                       "tool_input": {"command": "cargo test -q"}, "brand_new_field": [1,2]});
        let e = HookEvent::parse(&v).unwrap();
        assert!(is_test_command(e.bash_command().unwrap()));
        assert_eq!(e.tool_summary().unwrap(), "Bash: cargo test -q");
        assert!(HookEvent::parse(&json!({"foo": 1})).is_none());
    }
}
