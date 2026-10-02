//! What an agent changed during a session, in any folder (git or not).
//!
//! Just before Claude Code edits a file (the `PreToolUse` hook for Edit,
//! MultiEdit, Write or NotebookEdit), Promptly keeps a copy of the file as it
//! was. The session's changes are those copies diffed against the files on
//! disk now, so they show up live, while the agent works, and outside git
//! repositories too.

use crate::git::{self, FileDiff, FileStatus, ReviewDiff};
use crate::hooks::HookEvent;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Files above this size are listed but not diffed.
const MAX_BASELINE: u64 = 4 * 1024 * 1024;

/// A file as it was before the agent first touched it.
#[derive(Debug, Clone, PartialEq)]
pub enum Baseline {
    /// It did not exist (the agent created it).
    Missing,
    Content(Arc<Vec<u8>>),
    /// Too large to keep; listed without a diff.
    TooLarge,
}

pub type Baselines = BTreeMap<PathBuf, Baseline>;

/// The file a file-editing tool is about to touch, made absolute.
pub fn edited_path(ev: &HookEvent) -> Option<PathBuf> {
    if !ev.is_file_change() {
        return None;
    }
    let input = ev.tool_input.as_ref()?;
    let p = input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))?
        .as_str()?;
    let p = PathBuf::from(p);
    Some(if p.is_absolute() {
        p
    } else {
        ev.cwd.clone()?.join(p)
    })
}

/// Read a file's current content as its baseline.
pub fn capture(path: &Path) -> Baseline {
    match std::fs::metadata(path) {
        Err(_) => Baseline::Missing,
        Ok(m) if m.len() > MAX_BASELINE => Baseline::TooLarge,
        Ok(_) => match std::fs::read(path) {
            Ok(b) => Baseline::Content(Arc::new(b)),
            Err(_) => Baseline::Missing,
        },
    }
}

/// Record the baseline for the file this event is about to edit, once per
/// file per session. Returns true when a new file was recorded.
pub fn record(baselines: &mut Baselines, ev: &HookEvent) -> bool {
    if ev.hook_event_name != "PreToolUse" {
        return false;
    }
    let Some(path) = edited_path(ev) else {
        return false;
    };
    if baselines.contains_key(&path) {
        return false;
    }
    let b = capture(&path);
    baselines.insert(path, b);
    true
}

/// The session's changes as a review diff. Paths under `root` are shown
/// relative to it. Files that are back to their baseline are left out.
pub fn diff(root: &Path, baselines: &Baselines) -> Result<ReviewDiff, String> {
    let tmp = TempDir::new().map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for (i, (path, base)) in baselines.iter().enumerate() {
        let shown = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let exists = path.exists();
        let base_file = match base {
            Baseline::Missing => None,
            Baseline::Content(bytes) => {
                let f = tmp.path().join(format!("base-{i}"));
                std::fs::write(&f, bytes.as_slice()).map_err(|e| e.to_string())?;
                Some(f)
            }
            Baseline::TooLarge => {
                files.push(FileDiff {
                    path: shown,
                    status: if exists {
                        FileStatus::Modified
                    } else {
                        FileStatus::Deleted
                    },
                    binary: true,
                    ..Default::default()
                });
                continue;
            }
        };
        if base_file.is_none() && !exists {
            continue; // created and removed again
        }
        let null = PathBuf::from("/dev/null");
        let a = base_file.clone().unwrap_or_else(|| null.clone());
        let b = if exists { path.clone() } else { null };
        let out = std::process::Command::new("git")
            .args([
                "diff",
                "--no-index",
                "--no-color",
                "--no-ext-diff",
                "-U3",
                "--",
            ])
            .arg(&a)
            .arg(&b)
            .output()
            .map_err(|e| format!("git is needed to show changes: {e}"))?;
        // Exit 0: identical, 1: different, anything else: error.
        match out.status.code() {
            Some(0) => continue,
            Some(1) => {}
            _ => return Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let Some(mut f) = git::parse_unified(&text).into_iter().next() else {
            continue;
        };
        f.path = shown;
        f.old_path = None;
        f.untracked = false;
        f.status = match (base_file.is_some(), exists) {
            (false, _) => FileStatus::Added,
            (true, false) => FileStatus::Deleted,
            (true, true) => FileStatus::Modified,
        };
        files.push(f);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ReviewDiff {
        root: root.to_path_buf(),
        files,
        scope: git::DiffScope::Session,
        branch: git::current_branch(root),
        base: None,
        ahead: 0,
    })
}

/// A private scratch folder removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p =
            std::env::temp_dir().join(format!("promptly-baseline-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p)?;
        Ok(Self(p))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pre(tool: &str, input: serde_json::Value, cwd: &Path) -> HookEvent {
        serde_json::from_value(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": input,
            "cwd": cwd,
        }))
        .unwrap()
    }

    #[test]
    fn records_once_and_diffs_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "a = 1\nb = 2\n").unwrap();
        std::fs::write(root.join("same.txt"), "unchanged\n").unwrap();
        let mut base = Baselines::new();

        assert!(record(
            &mut base,
            &pre("Edit", json!({"file_path": "app.py"}), root)
        ));
        // Second edit of the same file keeps the first baseline.
        std::fs::write(root.join("app.py"), "a = 1\nb = 3\n").unwrap();
        assert!(!record(
            &mut base,
            &pre("Edit", json!({"file_path": root.join("app.py")}), root)
        ));
        assert!(record(
            &mut base,
            &pre("Write", json!({"file_path": root.join("new.md")}), root)
        ));
        assert!(record(
            &mut base,
            &pre("Edit", json!({"file_path": root.join("same.txt")}), root)
        ));
        // Not a file edit.
        assert!(!record(
            &mut base,
            &pre("Bash", json!({"command": "ls"}), root)
        ));

        std::fs::write(root.join("new.md"), "# hi\n").unwrap();
        let d = diff(root, &base).unwrap();
        let names: Vec<_> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["app.py", "new.md"], "unchanged files are left out");
        let app = &d.files[0];
        assert_eq!(
            (app.status, app.added, app.removed),
            (FileStatus::Modified, 1, 1)
        );
        let new = &d.files[1];
        assert_eq!((new.status, new.added), (FileStatus::Added, 1));

        std::fs::remove_file(root.join("app.py")).unwrap();
        let d = diff(root, &base).unwrap();
        assert_eq!(d.files[0].status, FileStatus::Deleted);
    }
}
