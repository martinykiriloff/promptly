//! `~/.config/promptly/config.toml`: appearance, Claude launch defaults,
//! per-project profiles, keybindings and composer snippets.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font_size: f32,
    pub scrollback_lines: usize,
    /// Explicit shell; defaults to `$SHELL`.
    pub shell: Option<String>,
    pub shell_integration: bool,
    pub claude: ClaudeConfig,
    pub notifications: NotificationConfig,
    /// Per-project profiles, matched by the longest `root` prefix of the cwd.
    pub profiles: Vec<Profile>,
    /// Action name -> shortcut, e.g. `"palette" = "cmd+shift+p"`.
    pub keybindings: BTreeMap<String, String>,
    pub snippets: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            scrollback_lines: 100_000,
            shell: None,
            shell_integration: true,
            claude: ClaudeConfig::default(),
            notifications: NotificationConfig::default(),
            profiles: vec![],
            keybindings: BTreeMap::new(),
            snippets: BTreeMap::from([
                (
                    "review".into(),
                    "Review the diff on this branch for bugs and missing tests.".into(),
                ),
                (
                    "tests".into(),
                    "Run the test suite and fix any failures.".into(),
                ),
            ]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    /// Path or name of the `claude` binary.
    pub binary: String,
    /// Inject session-scoped hooks (channel 2). Off = transcript-only mode.
    pub inject_hooks: bool,
    /// Wrap the status line to read cost and context %.
    pub wrap_statusline: bool,
    pub extra_args: Vec<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            binary: "claude".into(),
            inject_hooks: true,
            wrap_statusline: true,
            extra_args: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub on_finish: bool,
    pub max_per_minute: u32,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_finish: true,
            max_per_minute: 6,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub root: PathBuf,
    pub env: BTreeMap<String, String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub mcp_config: Option<PathBuf>,
    /// Command run in new plain-shell panes, e.g. `nvm use`.
    pub startup_command: Option<String>,
    /// Create a git worktree for each new Claude session.
    pub worktree_per_session: bool,
}

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    /// Load the config; a malformed file yields defaults plus an error to show.
    pub fn load() -> (Self, Option<String>) {
        Self::load_from(&Self::path())
    }

    pub fn load_from(p: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(p) {
            Ok(s) => match toml::from_str(&s) {
                Ok(c) => (c, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", p.display()))),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn profile_for(&self, cwd: &Path) -> Option<&Profile> {
        self.profiles
            .iter()
            .filter(|p| !p.root.as_os_str().is_empty() && cwd.starts_with(expand_tilde(&p.root)))
            .max_by_key(|p| p.root.components().count())
    }

    pub fn binding(&self, action: &str) -> Option<&str> {
        self.keybindings.get(action).map(String::as_str)
    }
}

pub fn expand_tilde(p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => crate::paths::home().join(rest),
        Err(_) => p.to_path_buf(),
    }
}

/// Build the `claude` argv for a profile.
pub fn claude_args(cfg: &Config, profile: Option<&Profile>) -> Vec<String> {
    let mut args = cfg.claude.extra_args.clone();
    if let Some(p) = profile {
        if let Some(m) = &p.model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(m) = &p.permission_mode {
            args.extend(["--permission-mode".into(), m.clone()]);
        }
        if let Some(m) = &p.mcp_config {
            args.extend([
                "--mcp-config".into(),
                expand_tilde(m).to_string_lossy().into(),
            ]);
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_pick_longest_root_and_build_args() {
        let cfg: Config = toml::from_str(
            r#"
            [[profiles]]
            name = "all"
            root = "/work"
            [[profiles]]
            name = "client"
            root = "/work/client"
            model = "sonnet"
            permission_mode = "plan"
            env = { AWS_PROFILE = "client" }
            "#,
        )
        .unwrap();
        let p = cfg.profile_for(Path::new("/work/client/api")).unwrap();
        assert_eq!(p.name, "client");
        assert_eq!(
            claude_args(&cfg, Some(p)),
            ["--model", "sonnet", "--permission-mode", "plan"]
        );
        assert_eq!(
            cfg.profile_for(Path::new("/work/other")).unwrap().name,
            "all"
        );
        assert!(cfg.profile_for(Path::new("/elsewhere")).is_none());
        assert_eq!(cfg.font_size, 13.0, "defaults fill missing keys");
    }

    #[test]
    fn malformed_config_falls_back() {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), "font_size = 'big'").unwrap();
        let (c, err) = Config::load_from(t.path());
        assert!(err.is_some());
        assert_eq!(c.font_size, 13.0);
    }
}
