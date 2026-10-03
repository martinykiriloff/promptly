//! Local models served by Ollama, for running Claude Code on them.
//!
//! Ollama speaks Anthropic's Messages API, so Claude Code works against it
//! unchanged (see `Account::session_env`). Claude Code's system prompt alone
//! is about 25k tokens, so the server needs a context window of at least
//! 32k; when Promptly starts Ollama it sets one.

use crate::accounts::LocalModel;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Context window Promptly asks for when it starts Ollama.
pub const OLLAMA_CONTEXT: &str = "32768";

/// Ollama's address: `OLLAMA_HOST` if set, else localhost:11434.
pub fn ollama_base() -> String {
    base_from(std::env::var("OLLAMA_HOST").ok().as_deref())
}

fn base_from(host: Option<&str>) -> String {
    let h = host
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or("127.0.0.1:11434");
    let h = h
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    // A server bound to all interfaces is reached on localhost.
    let h = h.replacen("0.0.0.0", "127.0.0.1", 1);
    let h = if h.contains(':') {
        h
    } else {
        format!("{h}:11434")
    };
    format!("http://{h}")
}

/// Minimal HTTP GET for a localhost JSON endpoint; `None` when unreachable.
fn get(base: &str, path: &str, timeout: Duration) -> Option<String> {
    let hostport = base.trim_start_matches("http://");
    let addr = hostport.to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&addr, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: {hostport}\r\nAccept: application/json\r\n\r\n"
    )
    .ok()?;
    let mut buf = Vec::new();
    s.take(8 * 1024 * 1024).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.").then_some(())?;
    head.split_whitespace()
        .nth(1)
        .filter(|c| *c == "200")
        .map(|_| body.to_string())
}

pub fn ollama_running(base: &str) -> bool {
    get(base, "/api/version", Duration::from_millis(300)).is_some()
}

/// Model names from a running server's `/api/tags`.
fn models_from_tags(json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let mut out: Vec<String> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["name"].as_str().map(str::to_owned))
        // Embedding models can't chat.
        .filter(|n| !n.contains("embed"))
        .collect();
    out.sort();
    out
}

/// Model names from Ollama's model folder, for when the server is stopped:
/// `manifests/<registry>/<namespace>/<name>/<tag>`.
fn models_on_disk(models_dir: &Path) -> Vec<String> {
    let root = models_dir.join("manifests");
    let mut out = vec![];
    let Ok(registries) = std::fs::read_dir(&root) else {
        return out;
    };
    for reg in registries.flatten() {
        for ns in std::fs::read_dir(reg.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            for name in std::fs::read_dir(ns.path()).into_iter().flatten().flatten() {
                for tag in std::fs::read_dir(name.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let ns_name = ns.file_name().to_string_lossy().into_owned();
                    let model = name.file_name().to_string_lossy().into_owned();
                    let tag = tag.file_name().to_string_lossy().into_owned();
                    let full = if ns_name == "library" {
                        format!("{model}:{tag}")
                    } else {
                        format!("{ns_name}/{model}:{tag}")
                    };
                    if !full.contains("embed") {
                        out.push(full);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn ollama_models_dir() -> PathBuf {
    std::env::var_os("OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".ollama/models"))
}

/// The `ollama` binary, if installed.
pub fn ollama_binary() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // GUI apps get a short PATH; look where installers put it too.
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/Applications/Ollama.app/Contents/Resources",
        ]
        .iter()
        .map(PathBuf::from),
    );
    dirs.into_iter()
        .map(|d| d.join("ollama"))
        .find(|p| p.is_file())
}

/// Every local model available: from the running server, or from Ollama's
/// model folder when it is installed but stopped.
pub fn discover() -> Vec<LocalModel> {
    let base = ollama_base();
    let names = match get(&base, "/api/tags", Duration::from_millis(400)) {
        Some(json) => models_from_tags(&json),
        None if ollama_binary().is_some() => models_on_disk(&ollama_models_dir()),
        None => vec![],
    };
    names
        .into_iter()
        .map(|model| LocalModel {
            provider: "Ollama".into(),
            base_url: base.clone(),
            model,
        })
        .collect()
}

/// Make sure Ollama is serving, starting it (with a context window large
/// enough for Claude Code) if needed. Ok(true) when Promptly started it.
pub fn ensure_ollama(base: &str) -> Result<bool, String> {
    if ollama_running(base) {
        return Ok(false);
    }
    let bin = ollama_binary().ok_or("Ollama isn't installed (brew install ollama)")?;
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if std::env::var_os("OLLAMA_CONTEXT_LENGTH").is_none() {
        cmd.env("OLLAMA_CONTEXT_LENGTH", OLLAMA_CONTEXT);
    }
    // Its own process group: it keeps serving after Promptly quits, like
    // the Ollama app does.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
        .map_err(|e| format!("Couldn't start Ollama: {e}"))?;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) {
        if ollama_running(base) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("Ollama didn't start within 10 seconds".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_forms() {
        assert_eq!(base_from(None), "http://127.0.0.1:11434");
        assert_eq!(base_from(Some("0.0.0.0")), "http://127.0.0.1:11434");
        assert_eq!(
            base_from(Some("http://localhost:9999/")),
            "http://localhost:9999"
        );
        assert_eq!(base_from(Some("10.0.0.5")), "http://10.0.0.5:11434");
    }

    #[test]
    fn parses_tags_and_disk() {
        let tags = r#"{"models":[{"name":"qwen3:8b"},{"name":"nomic-embed-text:latest"},{"name":"llama3.2:3b"}]}"#;
        assert_eq!(models_from_tags(tags), ["llama3.2:3b", "qwen3:8b"]);

        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("manifests/registry.ollama.ai");
        std::fs::create_dir_all(m.join("library/qwen3")).unwrap();
        std::fs::write(m.join("library/qwen3/8b"), "{}").unwrap();
        std::fs::create_dir_all(m.join("someone/coder")).unwrap();
        std::fs::write(m.join("someone/coder/latest"), "{}").unwrap();
        assert_eq!(
            models_on_disk(d.path()),
            ["qwen3:8b", "someone/coder:latest"]
        );
    }

    #[test]
    fn local_account_env() {
        use crate::accounts::Account;
        let home = Path::new("/home/me");
        let a = Account::local(
            LocalModel {
                provider: "Ollama".into(),
                base_url: "http://127.0.0.1:11434".into(),
                model: "qwen3:8b".into(),
            },
            home,
        );
        assert_eq!(a.id, "local:ollama:qwen3:8b");
        assert_eq!(a.short_label(), "qwen3:8b");
        assert_eq!(a.env_value(), None, "default config folder");
        assert!(!a.owns(Path::new("/home/me/.claude/projects/x.jsonl")));
        let env: std::collections::HashMap<_, _> = a.session_env().into_iter().collect();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:11434");
        assert_eq!(env["ANTHROPIC_API_KEY"], "");
        assert_eq!(env["ANTHROPIC_MODEL"], "qwen3:8b");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "qwen3:8b");
    }

    /// Against the real server, when one is running (skipped otherwise).
    #[test]
    fn live_ollama_if_running() {
        let base = ollama_base();
        if !ollama_running(&base) {
            return;
        }
        let models = discover();
        assert!(models.iter().all(|m| m.provider == "Ollama"));
    }
}
