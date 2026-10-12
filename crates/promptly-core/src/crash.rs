//! Crash reports, opt-in: a panic is written to a local file; on the next
//! launch the user can open a prefilled GitHub issue or dismiss it. Nothing
//! is ever sent automatically.

use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("crashes")
}

/// The report text for a panic (home folder shown as `~`).
pub fn report(message: &str, location: Option<&str>, backtrace: &str, version: &str) -> String {
    let home = crate::paths::home().to_string_lossy().to_string();
    let text = format!(
        "Promptly {version} on {} {}\n\nPanic: {message}\nAt: {}\n\nBacktrace:\n{backtrace}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        location.unwrap_or("unknown"),
    );
    if home.len() > 1 {
        text.replace(&home, "~")
    } else {
        text
    }
}

/// Save a report in `dir`; returns its path.
pub fn save_in(dir: &Path, report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let p = dir.join(format!("crash-{secs}.txt"));
    crate::util::write_private(&p, report.as_bytes())?;
    Ok(p)
}

/// Reports not yet reported or dismissed, oldest first.
pub fn pending_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    v.sort();
    v
}

/// Mark reports as handled (kept on disk as `.seen`).
pub fn dismiss(paths: &[PathBuf]) {
    for p in paths {
        let _ = std::fs::rename(p, p.with_extension("seen"));
    }
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A "new issue" link with the report filled in (trimmed to fit a URL).
pub fn issue_url(repo: &str, report: &str) -> String {
    let first = report
        .lines()
        .find_map(|l| l.strip_prefix("Panic: "))
        .unwrap_or("crash");
    let title: String = format!("Crash: {first}").chars().take(120).collect();
    let mut body: String = report.chars().take(5000).collect();
    if body.len() < report.len() {
        body.push_str("\n… (trimmed)");
    }
    let body = format!("**What were you doing?**\n\n\n**Report**\n```\n{body}\n```\n");
    format!(
        "https://github.com/{repo}/issues/new?title={}&body={}",
        encode(&title),
        encode(&body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_dismiss_and_link() {
        let d = tempfile::tempdir().unwrap();
        let r = report(
            "index out of bounds",
            Some("src/app.rs:10:5"),
            "0: main",
            "2026.10.0",
        );
        assert!(r.contains("Promptly 2026.10.0"));
        assert!(r.contains("At: src/app.rs:10:5"));
        let p = save_in(d.path(), &r).unwrap();
        assert_eq!(pending_in(d.path()), std::slice::from_ref(&p));
        let url = issue_url("me/promptly", &r);
        assert!(url.starts_with("https://github.com/me/promptly/issues/new?title=Crash%3A%20index%20out%20of%20bounds&body="));
        assert!(!url.contains(' ') && !url.contains('\n'));
        dismiss(&[p]);
        assert!(pending_in(d.path()).is_empty());
        let long = "x".repeat(20_000);
        assert!(issue_url("me/p", &long).len() < 20_000);
    }
}
