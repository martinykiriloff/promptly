//! `promptly-ctl`: the hook binary Claude Code calls, and the scriptable CLI.
//!
//! Hook mode must never break Claude Code: it always exits 0, prints
//! nothing, and gives up after a few milliseconds if Promptly is not there.

use promptly_core::env;
use promptly_core::ipc::{self, ControlRequest, Envelope};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const HOOK_BUDGET: Duration = Duration::from_millis(15);
const MAX_STDIN: u64 = 4 * 1024 * 1024;

const USAGE: &str = "\
promptly-ctl: control a running Promptly

USAGE:
  promptly-ctl new-session [--claude] [--cwd DIR] [--worktree NAME] [--prompt TEXT]
  promptly-ctl notify TITLE [BODY]
  promptly-ctl list
  promptly-ctl focus SESSION
  promptly-ctl action NAME        run a palette action (usage, toggle_grid, new_claude, ...)

Internal (invoked by Claude Code via injected settings):
  promptly-ctl hook
  promptly-ctl statusline
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => {
            hook();
            ExitCode::SUCCESS
        }
        Some("statusline") => {
            statusline();
            ExitCode::SUCCESS
        }
        Some("new-session") => control(parse_new_session(&args[1..])),
        Some("notify") if args.len() >= 2 => control(Ok(ControlRequest::Notify {
            title: args[1].clone(),
            body: args.get(2).cloned().unwrap_or_default(),
        })),
        Some("list") => control(Ok(ControlRequest::ListSessions)),
        Some("action") if args.len() == 2 => control(Ok(ControlRequest::Action {
            name: args[1].clone(),
        })),
        Some("focus") if args.len() == 2 => control(Ok(ControlRequest::Focus {
            session: args[1].clone(),
        })),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn read_stdin() -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf);
    buf
}

fn session_target() -> Option<(PathBuf, String)> {
    let sock = std::env::var_os(env::SOCKET)?;
    let token = std::env::var(env::TOKEN).ok()?;
    Some((PathBuf::from(sock), token))
}

/// Forward a hook payload. Outside Promptly (no env) this is a no-op.
fn hook() {
    let input = read_stdin();
    let Some((sock, token)) = session_target() else {
        return;
    };
    let Ok(payload) = serde_json::from_slice(&input) else {
        return;
    };
    let _ = ipc::send_oneshot(&sock, &Envelope::Hook { token, payload }, HOOK_BUDGET);
}

/// Forward status line input, then chain to the user's own status line
/// command so wrapping is invisible.
fn statusline() {
    let input = read_stdin();
    if let Some((sock, token)) = session_target()
        && let Ok(payload) = serde_json::from_slice(&input)
    {
        let _ = ipc::send_oneshot(&sock, &Envelope::Statusline { token, payload }, HOOK_BUDGET);
    }
    let Ok(user_cmd) = std::env::var(env::USER_STATUSLINE) else {
        return;
    };
    if user_cmd.trim().is_empty() {
        return;
    }
    let Ok(mut child) = Command::new("/bin/sh")
        .arg("-c")
        .arg(&user_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
    }
    if let Ok(out) = child.wait_with_output() {
        let _ = std::io::stdout().write_all(&out.stdout);
    }
}

fn parse_new_session(args: &[String]) -> Result<ControlRequest, String> {
    let mut cwd = None;
    let mut claude = false;
    let mut worktree = None;
    let mut prompt = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--claude" => claude = true,
            "--cwd" => cwd = Some(PathBuf::from(it.next().ok_or("--cwd needs a value")?)),
            "--worktree" => worktree = Some(it.next().ok_or("--worktree needs a value")?.clone()),
            "--prompt" => prompt = Some(it.next().ok_or("--prompt needs a value")?.clone()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let cwd = cwd
        .or_else(|| std::env::current_dir().ok())
        .map(|p| p.canonicalize().unwrap_or(p));
    if prompt.is_some() || worktree.is_some() {
        claude = true;
    }
    Ok(ControlRequest::NewSession {
        cwd,
        claude,
        worktree,
        prompt,
    })
}

fn control(req: Result<ControlRequest, String>) -> ExitCode {
    let req = match req {
        Ok(r) => r,
        Err(e) => {
            eprintln!("promptly-ctl: {e}");
            return ExitCode::from(2);
        }
    };
    let path = match std::env::var_os(env::CONTROL) {
        Some(p) => PathBuf::from(p),
        None => match ipc::control_socket_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("promptly-ctl: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    match ipc::request(&path, req) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            if v["ok"].as_bool() == Some(true) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!(
                "promptly-ctl: cannot reach Promptly at {}: {e}",
                path.display()
            );
            ExitCode::FAILURE
        }
    }
}
