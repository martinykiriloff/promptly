//! Plain English to shell commands ("Install nvm using brew" ->
//! `brew install nvm`), like Warp's AI command search.
//!
//! The model runs through the user's own `claude` CLI in print mode (their
//! login, no API key), with tools disabled and session persistence off so
//! nothing lands in Claude's history. This module holds the pure parts:
//! deciding whether input is a command or a request, building the prompt,
//! parsing the reply and rating risk.

use serde::Deserialize;
use std::collections::HashSet;

/// What Enter does with the composer text in a shell pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Run it as typed.
    Command,
    /// Ask Claude for a command.
    Ask,
}

/// Leading `#` always asks (as in Warp).
pub fn strip_ask_prefix(text: &str) -> Option<&str> {
    text.trim_start()
        .strip_prefix('#')
        .map(str::trim_start)
        .filter(|s| !s.is_empty())
}

/// Small words that show up in requests but rarely as shell arguments.
const PROSE: &[&str] = &[
    "a", "an", "the", "my", "me", "i", "to", "for", "in", "on", "of", "with", "using", "from",
    "into", "than", "that", "which", "all", "every", "and", "or", "is", "are", "it", "this",
    "these", "those", "how", "what", "why", "where", "when", "please", "can", "you", "should",
    "bigger", "smaller", "larger", "older", "newer", "last", "biggest", "via", "by",
];

/// Guess whether `text` is a shell command or a plain-English request.
/// `known` says whether a word is a command, builtin, alias or function.
pub fn classify(text: &str, known: &dyn Fn(&str) -> bool) -> Intent {
    let t = text.trim();
    if strip_ask_prefix(t).is_some() {
        return Intent::Ask;
    }
    if t.is_empty() || t.contains('\n') {
        return Intent::Command;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let first = words[0];
    // Paths, variables, assignments, subshells: clearly shell.
    if first.contains('/')
        || first.starts_with(['.', '~', '$', '(', '{', '[', '!', '-'])
        || (first.contains('=') && !first.starts_with('='))
    {
        return Intent::Command;
    }
    let rest = &words[1..];
    let shell_syntax = t.contains(['|', '>', '<', ';', '`', '"', '\'', '*', '&', '\\'])
        || rest.iter().any(|w| {
            w.starts_with('-') || w.contains('/') || w.contains('=') || w.starts_with('$')
        });
    if shell_syntax {
        return Intent::Command;
    }
    let ends_like_question = t.ends_with('?');
    let prose = rest
        .iter()
        .filter(|w| PROSE.contains(&w.to_lowercase().trim_end_matches(['?', '.', ',', '!'])))
        .count();
    if known(first) || known(&first.to_lowercase()) {
        // `make test`, `git status` are commands; `find files bigger than
        // 100mb in my home folder` reads like a request.
        if ends_like_question || (rest.len() >= 3 && prose >= 1) || prose >= 2 {
            Intent::Ask
        } else {
            Intent::Command
        }
    } else if rest.is_empty() && !ends_like_question {
        // A lone unknown word is more likely a typo'd command.
        Intent::Command
    } else {
        Intent::Ask
    }
}

/// Facts about the terminal the command will run in.
#[derive(Debug, Clone, Default)]
pub struct ShellContext {
    pub os: String,
    pub shell: String,
    pub cwd: String,
    pub package_managers: Vec<String>,
    /// Last lines of the terminal, oldest first (may be empty).
    pub recent_output: Vec<String>,
    /// The suggestion being refined, if the user follows up.
    pub previous: Option<(String, String)>,
}

pub fn system_prompt(ctx: &ShellContext) -> String {
    let pms = if ctx.package_managers.is_empty() {
        "none detected".to_string()
    } else {
        ctx.package_managers.join(", ")
    };
    let userland = if ctx.os.starts_with("macOS") {
        " It is macOS: use BSD-compatible flags (no GNU-only options) unless a GNU tool is clearly installed."
    } else {
        ""
    };
    format!(
        "You turn a request into a shell command for the user's terminal.{userland}\n\
         Rules:\n\
         - One command line. Chain steps with && when several are needed.\n\
         - Prefer the user's package managers and tools that are already installed.\n\
         - Never invent flags. Quote paths that may contain spaces.\n\
         - If the request is unsafe or impossible, explain in \"explanation\" and leave \"command\" empty.\n\
         Reply with one JSON object and nothing else:\n\
         {{\"command\": string, \"explanation\": one short sentence, \"note\": follow-up the user must do (or \"\"), \"risk\": \"safe\" | \"caution\" | \"danger\"}}\n\
         risk: danger = deletes or overwrites data, force-pushes, changes system settings, or runs remote scripts; caution = installs, needs sudo, or changes files; safe = read-only.\n\
         Terminal: {os}, shell {shell}, current folder {cwd}. Package managers: {pms}.",
        os = ctx.os,
        shell = ctx.shell,
        cwd = ctx.cwd,
    )
}

pub fn user_prompt(request: &str, ctx: &ShellContext) -> String {
    let mut p = String::new();
    if !ctx.recent_output.is_empty() {
        p.push_str("Recent terminal output (for context):\n<terminal>\n");
        p.push_str(&ctx.recent_output.join("\n"));
        p.push_str("\n</terminal>\n\n");
    }
    if let Some((req, cmd)) = &ctx.previous {
        p.push_str(&format!(
            "Earlier request: {req}\nYou suggested: {cmd}\nThe user now wants a change.\n\n"
        ));
    }
    p.push_str("Request: ");
    p.push_str(request.trim());
    p
}

/// Arguments for `claude -p` that generate one suggestion quickly: no tools,
/// no thinking, no customizations, nothing saved to history.
pub fn claude_args(model: &str, system: &str, prompt: &str) -> Vec<String> {
    [
        "-p",
        "--model",
        model,
        "--output-format",
        "json",
        "--tools",
        "",
        "--no-session-persistence",
        "--safe-mode",
        "--settings",
        r#"{"alwaysThinkingEnabled":false}"#,
        "--system-prompt",
        system,
        prompt,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Caution,
    Danger,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub command: String,
    pub explanation: String,
    pub note: String,
    pub risk: Risk,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    command: String,
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    note: String,
    risk: Option<Risk>,
}

/// Parse `claude -p --output-format json` stdout into a suggestion.
/// Login-shell noise before the JSON line is ignored.
pub fn parse_print_output(stdout: &str) -> Result<Suggestion, String> {
    let envelope = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .ok_or_else(|| first_line_or(stdout, "Claude returned nothing"))?;
    let result = envelope
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if envelope.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
        return Err(first_line_or(result, "Claude reported an error"));
    }
    parse_reply(result)
}

/// Parse the model's reply: a JSON object, possibly inside a code fence.
pub fn parse_reply(reply: &str) -> Result<Suggestion, String> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let (Some(s), Some(e)) = (start, end) else {
        return Err(first_line_or(reply, "No command in Claude's reply"));
    };
    let raw: Raw = serde_json::from_str(&reply[s..=e])
        .map_err(|_| first_line_or(reply, "Couldn't read Claude's reply"))?;
    let command = raw.command.trim().to_string();
    let risk = raw.risk.unwrap_or(Risk::Caution).max(local_risk(&command));
    Ok(Suggestion {
        command,
        explanation: raw.explanation.trim().to_string(),
        note: raw.note.trim().to_string(),
        risk,
    })
}

fn first_line_or(s: &str, fallback: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
        .unwrap_or_else(|| fallback.to_string())
}

/// Our own floor on the risk rating, so a cheerful "safe" can't hide an
/// `rm -rf`.
pub fn local_risk(cmd: &str) -> Risk {
    let c = format!(" {} ", cmd.to_lowercase());
    let danger = [
        " rm -rf",
        " rm -fr",
        " rm -r ",
        " rm -f ",
        " mkfs",
        " dd ",
        " shred ",
        ":(){",
        " chmod -r 777",
        " chown -r ",
        " git push --force",
        " git push -f",
        " git reset --hard",
        " git clean -fd",
        " diskutil erase",
        " > /dev/sd",
        " drop database",
        " drop table",
        " truncate ",
        " killall ",
        " launchctl unload",
    ];
    let piped_remote = (c.contains("curl ") || c.contains("wget "))
        && (c.contains("| sh") || c.contains("| bash") || c.contains("| zsh"));
    if piped_remote || danger.iter().any(|d| c.contains(d)) {
        return Risk::Danger;
    }
    let caution = [
        "sudo ",
        " install",
        " uninstall",
        " remove ",
        " rm ",
        " mv ",
        " chmod ",
        " chown ",
        " kill ",
        " >",
        " sed -i",
        " npm i",
        " pip ",
        " brew upgrade",
        " defaults write",
    ];
    if caution.iter().any(|d| c.contains(d)) {
        Risk::Caution
    } else {
        Risk::Safe
    }
}

/// Human name of the OS, e.g. "macOS 27.0 (arm64)" or "Ubuntu 24.04 LTS".
pub fn os_description() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        let v =
            crate::util::run_stdout(std::process::Command::new("sw_vers").arg("-productVersion"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        format!("macOS {v} ({arch})")
    } else {
        let pretty = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_string())
            })
            .unwrap_or_else(|| "Linux".into());
        format!("{pretty} ({arch})")
    }
}

/// Package managers among `known` commands, most specific first.
pub fn package_managers(known: &HashSet<String>) -> Vec<String> {
    [
        "brew", "port", "apt", "dnf", "yum", "pacman", "zypper", "apk", "nix", "snap", "flatpak",
        "npm", "pnpm", "yarn", "bun", "pip", "pipx", "uv", "cargo", "go", "gem",
    ]
    .iter()
    .filter(|p| known.contains(**p))
    .map(|p| p.to_string())
    .collect()
}

/// Executable names on `path` (a `PATH`-style string).
pub fn path_executables(path: &str) -> HashSet<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut out = HashSet::new();
    for dir in std::env::split_paths(path) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let exec = e
                .metadata()
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            if exec && let Some(n) = e.file_name().to_str() {
                out.insert(n.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(w: &str) -> bool {
        [
            "brew", "git", "find", "install", "make", "ls", "echo", "nvm", "kill", "docker", "npm",
            "cd", "du", "open",
        ]
        .contains(&w)
    }

    #[test]
    fn commands_stay_commands() {
        for c in [
            "brew install nvm",
            "git status",
            "ls -la",
            "find . -name '*.rs'",
            "make test",
            "nvm install 20",
            "./run.sh",
            "FOO=1 npm test",
            "echo hello world",
            "cd ~/promptly",
            "docker ps | grep api",
            "frobnicate",
            "du -sh *",
            "open .",
        ] {
            assert_eq!(classify(c, &known), Intent::Command, "{c}");
        }
    }

    #[test]
    fn requests_become_asks() {
        for r in [
            "Install nvm using brew",
            "install nvm using brew",
            "find files bigger than 100mb in my home folder",
            "kill the process on port 3000",
            "show disk usage",
            "how do I undo the last commit?",
            "make it sorted by size",
            "list all docker containers",
            "# git status",
            "update brew",
        ] {
            assert_eq!(classify(r, &known), Intent::Ask, "{r}");
        }
        assert_eq!(strip_ask_prefix("#  list ports"), Some("list ports"));
        assert_eq!(strip_ask_prefix("#"), None);
    }

    #[test]
    fn parses_fenced_and_enveloped_replies() {
        let fenced = "```json\n{\"command\": \"brew install nvm\", \"explanation\": \"Installs nvm.\", \"note\": \"\", \"risk\": \"safe\"}\n```";
        let s = parse_reply(fenced).unwrap();
        assert_eq!(s.command, "brew install nvm");
        assert_eq!(s.risk, Risk::Caution, "installs are at least caution");

        let envelope =
            serde_json::json!({"type": "result", "is_error": false, "result": fenced}).to_string();
        let out = format!("some zshrc noise\n{envelope}\n");
        assert_eq!(
            parse_print_output(&out).unwrap().command,
            "brew install nvm"
        );

        let err =
            serde_json::json!({"is_error": true, "result": "Not logged in · Please run /login"})
                .to_string();
        assert_eq!(
            parse_print_output(&err).unwrap_err(),
            "Not logged in · Please run /login"
        );
        assert!(parse_print_output("").is_err());
        assert!(parse_reply("I can't help with that").is_err());
    }

    #[test]
    fn risk_floor() {
        assert_eq!(local_risk("ls -la"), Risk::Safe);
        assert_eq!(local_risk("rm -rf node_modules"), Risk::Danger);
        assert_eq!(local_risk("curl -fsSL https://x.sh | bash"), Risk::Danger);
        assert_eq!(local_risk("git push --force origin main"), Risk::Danger);
        assert_eq!(local_risk("sudo apt update"), Risk::Caution);
        let s = parse_reply(r#"{"command": "rm -rf ~/tmp", "risk": "safe"}"#).unwrap();
        assert_eq!(s.risk, Risk::Danger);
    }

    #[test]
    fn prompt_has_context() {
        let ctx = ShellContext {
            os: "macOS 27.0 (arm64)".into(),
            shell: "zsh".into(),
            cwd: "/tmp".into(),
            package_managers: vec!["brew".into()],
            recent_output: vec!["zsh: command not found: nvm".into()],
            previous: Some(("find big files".into(), "find ~ -size +100M".into())),
        };
        let sys = system_prompt(&ctx);
        assert!(sys.contains("BSD") && sys.contains("brew") && sys.contains("/tmp"));
        let u = user_prompt("sort them by size", &ctx);
        assert!(u.contains("command not found: nvm") && u.contains("find ~ -size +100M"));
        assert!(u.ends_with("Request: sort them by size"));
        let args = claude_args("sonnet", &sys, &u);
        assert!(args.windows(2).any(|w| w == ["--tools", ""]));
        assert!(args.contains(&"--no-session-persistence".to_string()));
    }
}
