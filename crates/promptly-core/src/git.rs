//! Git integration. Shells out to `git` so behavior matches the user's own
//! git exactly (config, hooks, attributes).

use crate::util::run_stdout;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    c
}

pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    run_stdout(git(dir).args(["rev-parse", "--show-toplevel"])).map(PathBuf::from)
}

pub fn current_branch(dir: &Path) -> Option<String> {
    // symbolic-ref also works on an unborn branch (fresh repo, no commits).
    if let Some(b) = run_stdout(git(dir).args(["symbolic-ref", "--quiet", "--short", "HEAD"])) {
        return Some(b);
    }
    let b = run_stdout(git(dir).args(["rev-parse", "--abbrev-ref", "HEAD"]))?;
    if b == "HEAD" {
        run_stdout(git(dir).args(["rev-parse", "--short", "HEAD"])).map(|s| format!("@{s}"))
    } else {
        Some(b)
    }
}

pub fn is_dirty(dir: &Path) -> Option<bool> {
    run_stdout(git(dir).args(["status", "--porcelain"])).map(|s| !s.is_empty())
}

/// `promptly/<slug>` branch naming; slug keeps `[a-z0-9-]`.
pub fn branch_slug(name: &str) -> String {
    let mut s: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(48).collect();
    if s.is_empty() { "session".into() } else { s }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub repo: PathBuf,
}

/// Where worktrees for `repo` are created: outside the repo, under app data,
/// so they never show up as untracked files in the main checkout.
pub fn worktrees_home(repo: &Path) -> PathBuf {
    let name = repo
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let hash = repo
        .to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
    crate::paths::data_dir()
        .join("worktrees")
        .join(format!("{name}-{:08x}", hash as u32))
}

/// Create a worktree on a new `promptly/<slug>` branch from HEAD. A numeric
/// suffix is added if the branch or directory already exists.
pub fn create_worktree(repo: &Path, name: &str) -> Result<Worktree, String> {
    let root = repo_root(repo)
        .ok_or_else(|| format!("{} is not inside a git repository", repo.display()))?;
    let home = worktrees_home(&root);
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    let base = branch_slug(name);
    for i in 0..100 {
        let slug = if i == 0 {
            base.clone()
        } else {
            format!("{base}-{i}")
        };
        let branch = format!("promptly/{slug}");
        let path = home.join(&slug);
        let exists = path.exists()
            || run_stdout(git(&root).args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ]))
            .is_some();
        if exists {
            continue;
        }
        let out = git(&root)
            .args(["worktree", "add", "-b", &branch])
            .arg(&path)
            .arg("HEAD")
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        return Ok(Worktree {
            path,
            branch,
            repo: root,
        });
    }
    Err("could not find a free worktree name".into())
}

/// Remove a worktree. Refuses when it has uncommitted changes; the branch is
/// always kept so committed work is never lost.
pub fn remove_worktree(wt: &Worktree) -> Result<(), String> {
    match is_dirty(&wt.path) {
        Some(true) => return Err("worktree has uncommitted changes; commit or stash first".into()),
        None if wt.path.exists() => return Err("could not read worktree status".into()),
        _ => {}
    }
    let out = git(&wt.repo)
        .args(["worktree", "remove"])
        .arg(&wt.path)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn list_worktrees(repo: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Some(out) = run_stdout(git(repo).args(["worktree", "list", "--porcelain"])) else {
        return vec![];
    };
    let mut v = Vec::new();
    let mut cur: Option<(PathBuf, Option<String>)> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(c) = cur.take() {
                v.push(c);
            }
            cur = Some((PathBuf::from(p), None));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/")
            && let Some(c) = cur.as_mut()
        {
            c.1 = Some(b.to_string());
        }
    }
    v.extend(cur);
    v
}

// ---------------------------------------------------------------- diffs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Meta,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileStatus {
    Added,
    #[default]
    Modified,
    Deleted,
    Renamed,
}

impl FileStatus {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileDiff {
    pub path: String,
    /// Previous path for renames.
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub added: u32,
    pub removed: u32,
    pub untracked: bool,
    pub binary: bool,
    pub lines: Vec<DiffLine>,
}

/// What the review compares against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffScope {
    /// Like a pull request: everything since the branch left its base,
    /// committed or not.
    #[default]
    Branch,
    /// Only changes not yet committed.
    Uncommitted,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReviewDiff {
    pub root: PathBuf,
    pub files: Vec<FileDiff>,
    /// The scope actually used (Branch falls back to Uncommitted on the base branch).
    pub scope: DiffScope,
    pub branch: Option<String>,
    pub base: Option<String>,
    /// Commits on this branch that the base doesn't have.
    pub ahead: u32,
}

impl ReviewDiff {
    pub fn totals(&self) -> (u32, u32) {
        self.files
            .iter()
            .fold((0, 0), |a, f| (a.0 + f.added, a.1 + f.removed))
    }
}

/// The branch a pull request would target: origin's default branch, else a
/// local or remote `main`/`master`.
pub fn default_base(dir: &Path) -> Option<String> {
    if let Some(b) = run_stdout(git(dir).args([
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])) {
        return Some(b);
    }
    ["main", "master", "origin/main", "origin/master"]
        .into_iter()
        .find(|r| run_stdout(git(dir).args(["rev-parse", "--verify", "--quiet", r])).is_some())
        .map(str::to_owned)
}

/// The review diff for `dir`. `Branch` compares the working tree with the
/// merge-base of HEAD and the base branch, so it shows every change a pull
/// request would, plus anything not committed yet.
pub fn review_diff(dir: &Path, scope: DiffScope) -> Result<ReviewDiff, String> {
    let root = repo_root(dir)
        .ok_or_else(|| format!("{} is not inside a git repository", dir.display()))?;
    let branch = current_branch(&root);
    let mut out = ReviewDiff {
        root: root.clone(),
        branch: branch.clone(),
        ..Default::default()
    };
    if scope == DiffScope::Branch
        && let Some(base) = default_base(&root)
    {
        let base_short = base.trim_start_matches("origin/");
        let on_base = branch
            .as_deref()
            .is_some_and(|b| b == base || b == base_short);
        if !on_base && let Some(mb) = run_stdout(git(&root).args(["merge-base", "HEAD", &base])) {
            out.files = diff_against(&root, Some(&mb))?;
            out.ahead =
                run_stdout(git(&root).args(["rev-list", "--count", &format!("{mb}..HEAD")]))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            out.base = Some(base);
            out.scope = DiffScope::Branch;
            return Ok(out);
        }
    }
    out.files = diff_against(&root, None)?;
    out.scope = DiffScope::Uncommitted;
    Ok(out)
}

/// Working tree vs `rev` (or HEAD), plus untracked files.
fn diff_against(root: &Path, rev: Option<&str>) -> Result<Vec<FileDiff>, String> {
    let has_head =
        run_stdout(git(root).args(["rev-parse", "--verify", "--quiet", "HEAD"])).is_some();
    let mut cmd = git(root);
    cmd.args([
        "-c",
        "core.quotepath=off",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "-M",
    ]);
    match (rev, has_head) {
        (Some(r), _) => {
            cmd.arg(r);
        }
        (None, true) => {
            cmd.arg("HEAD");
        }
        (None, false) => {
            cmd.arg("--cached");
        }
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let mut files = parse_unified(&String::from_utf8_lossy(&out.stdout));
    if let Some(list) =
        run_stdout(git(root).args(["ls-files", "--others", "--exclude-standard", "-z"]))
    {
        for rel in list.split('\0').filter(|s| !s.is_empty()) {
            files.push(untracked_diff(root, rel));
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Max bytes of an untracked file rendered in the review pane.
const MAX_UNTRACKED: u64 = 256 * 1024;

/// All changes in the working tree relative to HEAD, including untracked
/// files (rendered as additions without touching the index).
pub fn working_diff(dir: &Path) -> Result<Vec<FileDiff>, String> {
    review_diff(dir, DiffScope::Uncommitted).map(|d| d.files)
}

fn untracked_diff(root: &Path, rel: &str) -> FileDiff {
    let p = root.join(rel);
    let mut fd = FileDiff {
        path: rel.to_string(),
        status: FileStatus::Added,
        untracked: true,
        ..Default::default()
    };
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if size > MAX_UNTRACKED {
        fd.lines.push(DiffLine {
            kind: LineKind::Meta,
            text: format!("new file, {size} bytes (too large to show)"),
            old_no: None,
            new_no: None,
        });
        return fd;
    }
    match std::fs::read(&p).map(String::from_utf8) {
        Ok(Ok(text)) => {
            fd.lines.push(DiffLine {
                kind: LineKind::Meta,
                text: "new file".into(),
                old_no: None,
                new_no: None,
            });
            for (i, l) in text.lines().enumerate() {
                fd.added += 1;
                fd.lines.push(DiffLine {
                    kind: LineKind::Added,
                    text: l.to_string(),
                    old_no: None,
                    new_no: Some(i as u32 + 1),
                });
            }
        }
        _ => {
            fd.binary = true;
            fd.lines.push(DiffLine {
                kind: LineKind::Meta,
                text: "binary file".into(),
                old_no: None,
                new_no: None,
            });
        }
    }
    fd
}

pub fn parse_unified(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_else(|| rest.to_string());
            files.push(FileDiff {
                path,
                ..Default::default()
            });
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if line.starts_with("@@") {
            // @@ -a,b +c,d @@
            let nums: Vec<u32> = line
                .split(' ')
                .filter(|t| t.starts_with('-') || t.starts_with('+'))
                .filter_map(|t| t[1..].split(',').next()?.parse().ok())
                .collect();
            if nums.len() == 2 {
                old_no = nums[0];
                new_no = nums[1];
            }
            f.lines.push(DiffLine {
                kind: LineKind::Hunk,
                text: line.to_string(),
                old_no: None,
                new_no: None,
            });
        } else if let Some(t) = line.strip_prefix('+').filter(|_| !line.starts_with("+++")) {
            f.added += 1;
            f.lines.push(DiffLine {
                kind: LineKind::Added,
                text: t.to_string(),
                old_no: None,
                new_no: Some(new_no),
            });
            new_no += 1;
        } else if let Some(t) = line.strip_prefix('-').filter(|_| !line.starts_with("---")) {
            f.removed += 1;
            f.lines.push(DiffLine {
                kind: LineKind::Removed,
                text: t.to_string(),
                old_no: Some(old_no),
                new_no: None,
            });
            old_no += 1;
        } else if let Some(t) = line.strip_prefix(' ') {
            f.lines.push(DiffLine {
                kind: LineKind::Context,
                text: t.to_string(),
                old_no: Some(old_no),
                new_no: Some(new_no),
            });
            old_no += 1;
            new_no += 1;
        } else if line.starts_with("Binary files") {
            f.binary = true;
            f.lines.push(DiffLine {
                kind: LineKind::Meta,
                text: line.to_string(),
                old_no: None,
                new_no: None,
            });
        } else if line.starts_with("rename ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
        {
            if line.starts_with("new file") {
                f.status = FileStatus::Added;
            } else if line.starts_with("deleted file") {
                f.status = FileStatus::Deleted;
            } else if let Some(from) = line.strip_prefix("rename from ") {
                f.status = FileStatus::Renamed;
                f.old_path = Some(from.to_string());
            }
            f.lines.push(DiffLine {
                kind: LineKind::Meta,
                text: line.to_string(),
                old_no: None,
                new_no: None,
            });
        }
    }
    files
}

/// Pair removed/added runs side by side for the split view.
pub fn split_rows(lines: &[DiffLine]) -> Vec<(Option<&DiffLine>, Option<&DiffLine>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Removed | LineKind::Added => {
                let start = i;
                while i < lines.len() && lines[i].kind == LineKind::Removed {
                    i += 1;
                }
                let rem = &lines[start..i];
                let add_start = i;
                while i < lines.len() && lines[i].kind == LineKind::Added {
                    i += 1;
                }
                let add = &lines[add_start..i];
                for k in 0..rem.len().max(add.len()) {
                    rows.push((rem.get(k), add.get(k)));
                }
            }
            _ => {
                rows.push((Some(&lines[i]), Some(&lines[i])));
                i += 1;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
 }
";

    #[test]
    fn parse_and_split() {
        let f = parse_unified(DIFF);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "src/a.rs");
        assert_eq!((f[0].added, f[0].removed), (1, 1));
        let add = f[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Added)
            .unwrap();
        assert_eq!(add.new_no, Some(2));
        let rows = split_rows(&f[0].lines);
        assert!(
            rows.iter()
                .any(|(l, r)| l.map(|x| x.text.as_str()) == Some("    old();")
                    && r.map(|x| x.text.as_str()) == Some("    new();"))
        );
    }

    #[test]
    fn slug() {
        assert_eq!(branch_slug("Fix: the Login bug!!"), "fix-the-login-bug");
        assert_eq!(branch_slug("***"), "session");
    }

    #[test]
    fn worktree_lifecycle() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let g = |args: &[&str]| {
            assert!(
                git(&repo).args(args).output().unwrap().status.success(),
                "{args:?}"
            )
        };
        g(&["init", "-q"]);
        g(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        let data = t.path().join("data");
        // Keep test worktrees out of the real app-data dir.
        // SAFETY: single-threaded test setup; no other test reads this var.
        unsafe { std::env::set_var("PROMPTLY_DATA_DIR", &data) };
        let wt = create_worktree(&repo, "Task One").unwrap();
        assert_eq!(wt.branch, "promptly/task-one");
        assert!(wt.path.exists());
        let wt2 = create_worktree(&repo, "Task One").unwrap();
        assert_eq!(wt2.branch, "promptly/task-one-1");
        std::fs::write(wt.path.join("x.txt"), "hi\n").unwrap();
        let d = working_diff(&wt.path).unwrap();
        assert!(
            d.iter()
                .any(|f| f.untracked && f.path == "x.txt" && f.added == 1)
        );
        assert!(remove_worktree(&wt).is_err(), "dirty worktree is kept");
        std::fs::remove_file(wt.path.join("x.txt")).unwrap();
        remove_worktree(&wt).unwrap();
        assert!(!wt.path.exists());
        assert_eq!(list_worktrees(&repo).len(), 2);
    }

    #[test]
    fn branch_review_matches_a_pull_request() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("r");
        std::fs::create_dir(&repo).unwrap();
        let g = |args: &[&str]| {
            let o = git(&repo)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("keep.txt"), "a\nb\n").unwrap();
        std::fs::write(repo.join("old.txt"), "same content here\n").unwrap();
        std::fs::write(repo.join("gone.txt"), "bye\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-qm", "base"]);
        g(&["checkout", "-qb", "feature"]);
        std::fs::write(repo.join("keep.txt"), "a\nB\n").unwrap();
        g(&["mv", "old.txt", "new.txt"]);
        g(&["rm", "-q", "gone.txt"]);
        g(&["commit", "-qam", "feature work"]);
        std::fs::write(repo.join("fresh.txt"), "not committed\n").unwrap();

        let d = review_diff(&repo, DiffScope::Branch).unwrap();
        assert_eq!(d.scope, DiffScope::Branch);
        assert_eq!(d.base.as_deref(), Some("main"));
        assert_eq!(d.ahead, 1);
        let st = |p: &str| d.files.iter().find(|f| f.path == p).map(|f| f.status);
        assert_eq!(st("keep.txt"), Some(FileStatus::Modified));
        assert_eq!(st("new.txt"), Some(FileStatus::Renamed));
        assert_eq!(st("gone.txt"), Some(FileStatus::Deleted));
        assert_eq!(
            st("fresh.txt"),
            Some(FileStatus::Added),
            "uncommitted files are included"
        );
        assert_eq!(d.totals(), (2, 2));

        // Uncommitted-only scope sees just the new file.
        let u = review_diff(&repo, DiffScope::Uncommitted).unwrap();
        assert_eq!(u.files.len(), 1);

        // On the base branch itself, Branch falls back to uncommitted changes.
        g(&["stash", "-qu"]);
        g(&["checkout", "-q", "main"]);
        let m = review_diff(&repo, DiffScope::Branch).unwrap();
        assert_eq!(m.scope, DiffScope::Uncommitted);
        assert!(m.files.is_empty());

        let e = review_diff(t.path(), DiffScope::Branch).unwrap_err();
        assert!(e.contains("is not inside a git repository"), "{e}");
    }
}
