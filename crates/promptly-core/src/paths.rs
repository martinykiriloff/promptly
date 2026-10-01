//! Filesystem locations. Everything Promptly writes lives under its own
//! directories; the user's `~/.claude` tree is only ever read.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MAX_SOCKET_DIR: usize = 80;

/// Directory for sockets and other per-boot state. Created `0700` and
/// verified to be owned by the current user, so a hostile local process
/// cannot pre-create it.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut dir = base.join(format!("promptly-{}", current_uid()));
    // Unix socket paths are limited to ~104 bytes (macOS) / 108 (Linux);
    // leave room for "s-NNNN.sock" and fall back to /tmp when too deep.
    if dir.as_os_str().len() > MAX_SOCKET_DIR {
        dir = PathBuf::from(format!("/tmp/promptly-{}", current_uid()));
    }
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Durable app data: session index, worktrees, saved layout.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PROMPTLY_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::data_dir()
        .unwrap_or_else(|| home().join(".local/share"))
        .join("Promptly")
}

/// User configuration (`config.toml`). XDG layout on both platforms, which is
/// what a terminal-native audience expects.
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("promptly")
}

/// Root of Claude Code's state. Honors `CLAUDE_CONFIG_DIR` like the CLI does.
pub fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
}

pub fn claude_projects_dir() -> PathBuf {
    claude_dir().join("projects")
}

/// Claude Code stores transcripts under a directory named after the cwd with
/// every non-alphanumeric character replaced by `-`.
pub fn transcript_path_for(cwd: &Path, session_id: &str) -> PathBuf {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    claude_projects_dir()
        .join(encoded)
        .join(format!("{session_id}.jsonl"))
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
    {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a directory owned by the current user",
                dir.display()
            ),
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_path_encoding_matches_claude_code() {
        let p = transcript_path_for(Path::new("/private/tmp/claude-501/x.y"), "abc");
        assert!(p.ends_with("-private-tmp-claude-501-x-y/abc.jsonl"));
    }

    #[test]
    fn runtime_dir_stays_short_enough_for_sockets() {
        let long = format!("/tmp/{}", "x".repeat(120));
        // SAFETY: test-local env mutation, read immediately below.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &long) };
        let d = runtime_dir().unwrap();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        assert!(
            d.join("control.sock").as_os_str().len() < 100,
            "{}",
            d.display()
        );
    }

    #[test]
    fn private_dir_is_0700() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("rt");
        ensure_private_dir(&d).unwrap();
        assert_eq!(
            fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
