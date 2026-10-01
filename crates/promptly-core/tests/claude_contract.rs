//! Contract test against the installed `claude` CLI: injected hooks reach
//! our socket, the state machine reaches a terminal state, and the
//! transcript is where we expect it with a schema we can parse.
//!
//! Makes one tiny API call, so it is ignored by default:
//!   cargo build -p promptly-ctl && cargo test -p promptly-core --test claude_contract -- --ignored --nocapture

use promptly_core::hooks::{HookEvent, InjectedSettings};
use promptly_core::ipc::{Incoming, Server};
use promptly_core::state::{SessionState, StateTracker};
use promptly_core::transcript::TranscriptReader;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, mpsc};
use std::time::Duration;

fn ctl_path() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/promptly-ctl");
    assert!(
        p.exists(),
        "build promptly-ctl first: cargo build -p promptly-ctl"
    );
    p.canonicalize().unwrap()
}

#[test]
#[ignore]
fn hooks_statusline_and_transcript_contract() {
    let ver = Command::new("claude")
        .arg("--version")
        .output()
        .expect("claude on PATH");
    println!("claude {}", String::from_utf8_lossy(&ver.stdout).trim());

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let sock = cwd.join("s.sock");
    let token = promptly_core::util::random_token();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let _srv = Server::spawn(sock.clone(), Some(token.clone()), move |m| {
        tx.lock().unwrap().send(m).unwrap();
    })
    .unwrap();

    let settings = cwd.join("settings.json");
    InjectedSettings::build(&ctl_path(), true)
        .write_to(&settings)
        .unwrap();
    let session_id = promptly_core::util::new_uuid();
    let out = Command::new("claude")
        .current_dir(&cwd)
        .args(["-p", "Reply with exactly the word: ok", "--settings"])
        .arg(&settings)
        .args(["--session-id", &session_id, "--max-turns", "1"])
        .env(promptly_core::env::SOCKET, &sock)
        .env(promptly_core::env::TOKEN, &token)
        .output()
        .expect("run claude");
    println!(
        "exit={:?} stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).trim()
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut names = vec![];
    let mut tracker = StateTracker::new();
    let mut transcript = None;
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(3)) {
        if let Incoming::Hook(v) = m {
            let ev = HookEvent::parse(&v).expect("hook payload has hook_event_name");
            assert_eq!(ev.session_id.as_deref(), Some(session_id.as_str()));
            transcript = ev.transcript_path.clone().or(transcript);
            tracker.on_hook(&ev);
            names.push(ev.hook_event_name);
        }
    }
    println!("hooks received: {names:?}");
    for required in ["SessionStart", "UserPromptSubmit", "Stop"] {
        assert!(names.iter().any(|n| n == required), "missing {required}");
    }
    assert_eq!(tracker.state(), SessionState::Idle);

    let expected = promptly_core::paths::transcript_path_for(&cwd, &session_id);
    let transcript = transcript.expect("hooks carry transcript_path");
    assert_eq!(transcript, expected, "transcript path encoding");
    let mut r = TranscriptReader::new(transcript.clone());
    r.poll().unwrap();
    println!("transcript stats: {:?}", r.stats);
    assert_eq!(r.stats.session_id.as_deref(), Some(session_id.as_str()));
    assert!(r.stats.user_turns >= 1);
    assert!(r.stats.context_tokens > 0, "usage fields parsed");
    assert!(r.stats.model.is_some());
    // Leave no trace in ~/.claude/projects for this throwaway session.
    let _ = std::fs::remove_file(&transcript);
    let _ = std::fs::remove_dir(transcript.parent().unwrap());
}

/// PRD: hook commands must return in under 20 ms.
///   cargo build --release -p promptly-ctl && cargo test -p promptly-core --test claude_contract hook_latency -- --ignored --nocapture
#[test]
#[ignore]
fn hook_latency_budget() {
    let ctl = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/promptly-ctl");
    assert!(ctl.exists(), "build release promptly-ctl first");
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("s.sock");
    let token = promptly_core::util::random_token();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let _srv = Server::spawn(sock.clone(), Some(token.clone()), move |m| {
        tx.lock().unwrap().send(m).unwrap();
    })
    .unwrap();
    let payload =
        br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let mut times = vec![];
    for _ in 0..60 {
        let t = std::time::Instant::now();
        let mut child = Command::new(&ctl)
            .arg("hook")
            .env(promptly_core::env::SOCKET, &sock)
            .env(promptly_core::env::TOKEN, &token)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(payload).unwrap();
        assert!(child.wait().unwrap().success());
        times.push(t.elapsed());
    }
    times.sort();
    let received = rx.try_iter().count();
    let p50 = times[times.len() / 2];
    let p95 = times[times.len() * 95 / 100];
    println!(
        "hook round trip incl. process spawn: p50={p50:?} p95={p95:?}, delivered {received}/60"
    );
    assert_eq!(received, 60);
    assert!(p95 < Duration::from_millis(20), "p95 {p95:?} over budget");
}
