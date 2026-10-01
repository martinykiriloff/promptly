//! Claude accounts on this machine.
//!
//! Claude Code keeps one account per config directory: `~/.claude` by
//! default, or whatever `CLAUDE_CONFIG_DIR` points at (`~/.claude-work`,
//! ...). Credentials are stored per directory, so switching accounts is just
//! launching `claude` with a different `CLAUDE_CONFIG_DIR`. Nothing here
//! touches credentials: identity comes from the `oauthAccount` block of the
//! account's `.claude.json`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Environment variable Claude Code reads its config directory from.
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Stable id: the config directory path.
    pub id: String,
    pub dir: PathBuf,
    /// True for `~/.claude`, which Claude Code uses when the variable is
    /// unset. It must be launched *without* `CLAUDE_CONFIG_DIR`: setting it,
    /// even to `~/.claude`, makes Claude Code look up different credentials.
    pub is_home: bool,
    pub email: Option<String>,
    pub org: Option<String>,
    /// User-chosen name, if any.
    pub name: Option<String>,
}

impl Account {
    fn new(dir: PathBuf, home: &Path) -> Self {
        let is_home = dir == home.join(".claude");
        let mut a = Self {
            id: dir.to_string_lossy().into_owned(),
            dir,
            is_home,
            email: None,
            org: None,
            name: None,
        };
        a.reload_identity(home);
        a
    }

    /// Where Claude Code keeps this account's `.claude.json`.
    pub fn identity_file(&self, home: &Path) -> PathBuf {
        if self.is_home {
            home.join(".claude.json")
        } else {
            self.dir.join(".claude.json")
        }
    }

    /// Re-read email and organization (e.g. after `/login`).
    pub fn reload_identity(&mut self, home: &Path) {
        let v: Option<Value> = std::fs::read(self.identity_file(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let acct = v.as_ref().and_then(|v| v.get("oauthAccount"));
        let field = |k: &str| {
            acct.and_then(|a| a.get(k))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        self.email = field("emailAddress");
        // "jane@x.com's Organization" is Claude's placeholder for personal
        // accounts; it says nothing the email doesn't.
        self.org = field("organizationName").filter(|o| !o.ends_with("'s Organization"));
    }

    pub fn signed_in(&self) -> bool {
        self.email.is_some()
    }

    /// Full label: the custom name, else the email, else the folder.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| self.folder_name())
    }

    /// One-word tag for session rows: custom name, organization, or
    /// "Personal" for the default account.
    pub fn short_label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        if let Some(o) = &self.org {
            return o.clone();
        }
        if self.is_home {
            return "Personal".into();
        }
        let f = self.folder_name();
        let f = f.trim_start_matches(".claude-").trim_start_matches('.');
        let mut c = f.chars();
        match c.next() {
            Some(first) => first.to_uppercase().chain(c).collect(),
            None => f.to_string(),
        }
    }

    /// Secondary line in the switcher: organization or folder.
    pub fn detail(&self) -> String {
        match (&self.org, self.signed_in()) {
            (Some(o), _) => format!("{o} · {}", self.tilde_dir()),
            (None, true) => self.tilde_dir(),
            (None, false) => format!("Not signed in · {}", self.tilde_dir()),
        }
    }

    pub fn initial(&self) -> char {
        self.label()
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('?')
    }

    fn folder_name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.id.clone())
    }

    fn tilde_dir(&self) -> String {
        let home = crate::paths::home();
        match self.dir.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => self.dir.display().to_string(),
        }
    }

    /// Value for `CLAUDE_CONFIG_DIR`, or `None` when it must stay unset.
    pub fn env_value(&self) -> Option<String> {
        (!self.is_home).then(|| self.dir.to_string_lossy().into_owned())
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.dir.join("projects")
    }

    /// Transcript path for a session started in `cwd` under this account.
    pub fn transcript_path_for(&self, cwd: &Path, session_id: &str) -> PathBuf {
        crate::paths::transcript_path_in(&self.projects_dir(), cwd, session_id)
    }

    /// Whether a transcript (or any file) lives under this account.
    pub fn owns(&self, path: &Path) -> bool {
        path.starts_with(&self.dir)
    }
}

/// Promptly's own record of accounts: extra folders and custom names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsFile {
    /// Config folders added by hand (anything not named `~/.claude-*`).
    #[serde(default)]
    pub extra: Vec<PathBuf>,
    /// Custom names by account id.
    #[serde(default)]
    pub names: std::collections::BTreeMap<String, String>,
    /// The account new sessions use.
    #[serde(default)]
    pub active: Option<String>,
}

impl AccountsFile {
    fn path() -> PathBuf {
        crate::paths::data_dir().join("accounts.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let p = Self::path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = crate::util::write_private(&p, &b);
        }
    }
}

/// Every account on the machine: `~/.claude` first, then each
/// `~/.claude-*` folder that Claude Code has used, then `extra` folders.
pub fn discover(home: &Path, file: &AccountsFile, inherited: Option<&Path>) -> Vec<Account> {
    let mut dirs = vec![home.join(".claude")];
    let mut found: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".claude-"))
                && p.is_dir()
                && looks_like_config_dir(p)
        })
        .collect();
    found.sort();
    dirs.extend(found);
    dirs.extend(inherited.map(Path::to_path_buf));
    dirs.extend(file.extra.iter().cloned());
    let mut out: Vec<Account> = vec![];
    for d in dirs {
        if out.iter().any(|a| same_dir(&a.dir, &d)) {
            continue;
        }
        let mut a = Account::new(d, home);
        a.name = file.names.get(&a.id).cloned();
        out.push(a);
    }
    out
}

/// A folder Claude Code has run in (or that Promptly created for it).
fn looks_like_config_dir(p: &Path) -> bool {
    [
        ".claude.json",
        "settings.json",
        "projects",
        ".promptly-account",
    ]
    .iter()
    .any(|f| p.join(f).exists())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

/// Create `~/.claude-<name>` for a new account. Claude Code asks to sign in
/// the first time it runs there.
pub fn create(home: &Path, name: &str) -> std::io::Result<PathBuf> {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "name needs a letter or digit",
        ));
    }
    let dir = home.join(format!(".claude-{slug}"));
    if dir.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("~/.claude-{slug} already exists"),
        ));
    }
    std::fs::create_dir_all(&dir)?;
    // Marks the folder as an account before Claude Code has written to it.
    std::fs::write(dir.join(".promptly-account"), b"")?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(email: &str, org: &str) -> String {
        serde_json::json!({
            "oauthAccount": {"emailAddress": email, "organizationName": org},
            "projects": {}
        })
        .to_string()
    }

    #[test]
    fn discovers_home_and_named_folders() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::write(
            h.join(".claude.json"),
            identity("me@gmail.com", "me@gmail.com's Organization"),
        )
        .unwrap();
        std::fs::create_dir_all(h.join(".claude-work")).unwrap();
        std::fs::write(
            h.join(".claude-work/.claude.json"),
            identity("me@acme.io", "Acme"),
        )
        .unwrap();
        // Not an account: no Claude files inside.
        std::fs::create_dir_all(h.join(".claude-backup-old")).unwrap();
        std::fs::write(h.join(".claude-notes.txt"), "x").unwrap();

        let accts = discover(h, &AccountsFile::default(), None);
        assert_eq!(accts.len(), 2);
        assert!(accts[0].is_home);
        assert_eq!(accts[0].email.as_deref(), Some("me@gmail.com"));
        assert_eq!(accts[0].org, None, "placeholder org is hidden");
        assert_eq!(accts[0].short_label(), "Personal");
        assert_eq!(
            accts[0].env_value(),
            None,
            "home account runs without the variable"
        );
        assert_eq!(accts[1].short_label(), "Acme");
        assert_eq!(accts[1].label(), "me@acme.io");
        assert_eq!(
            accts[1].env_value().as_deref(),
            Some(h.join(".claude-work").to_str().unwrap())
        );
    }

    #[test]
    fn names_extra_and_inherited_dedupe() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let other = tempfile::tempdir().unwrap();
        let mut file = AccountsFile::default();
        file.extra.push(other.path().to_path_buf());
        file.extra.push(h.join(".claude"));
        file.names
            .insert(other.path().to_string_lossy().into(), "Client".into());
        let accts = discover(h, &file, Some(other.path()));
        assert_eq!(accts.len(), 2);
        assert_eq!(accts[1].short_label(), "Client");
        assert!(!accts[1].signed_in());
        assert!(accts[1].detail().starts_with("Not signed in"));
    }

    #[test]
    fn create_makes_a_discoverable_folder() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let d = create(h, "Side Project!").unwrap();
        assert_eq!(d, h.join(".claude-side-project"));
        assert!(create(h, "side project").is_err(), "no clobbering");
        assert!(create(h, "  ").is_err());
        let accts = discover(h, &AccountsFile::default(), None);
        assert_eq!(accts[1].short_label(), "Side-project");
        assert_eq!(
            accts[1].transcript_path_for(Path::new("/a/b"), "s1"),
            d.join("projects/-a-b/s1.jsonl")
        );
    }
}
