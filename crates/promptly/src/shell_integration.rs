//! Auto-injected shell integration (OSC 133 prompt marks, exit codes, OSC 7
//! cwd) for zsh, bash and fish. The user's startup files still run; ours
//! are layered on top and can be disabled per profile.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ZSHENV: &str = r#"# Promptly shell integration: restore the user's ZDOTDIR, then load their env.
if [[ -n "${PROMPTLY_ORIG_ZDOTDIR+x}" ]]; then
  ZDOTDIR="$PROMPTLY_ORIG_ZDOTDIR"; unset PROMPTLY_ORIG_ZDOTDIR
else
  unset ZDOTDIR
fi
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  _promptly_precmd() {
    local ret=$?
    if [[ -n "$_promptly_cmd" ]]; then printf '\e]133;D;%s\a' "$ret"; fi
    _promptly_cmd=
    printf '\e]7;file://%s%s\a' "${HOST}" "${PWD// /%20}"
    printf '\e]133;A\a'
  }
  _promptly_preexec() { _promptly_cmd=1; printf '\e]133;C\a'; }
  add-zsh-hook precmd _promptly_precmd
  add-zsh-hook preexec _promptly_preexec
fi
"#;

const BASHRC: &str = r#"# Promptly shell integration for bash.
if [ -f /etc/profile ]; then . /etc/profile; fi
for f in ~/.bash_profile ~/.bash_login ~/.profile; do
  if [ -f "$f" ]; then . "$f"; break; fi
done
case "$-" in *i*)
  _promptly_cmd=
  _promptly_prompt() {
    local ret=$?
    if [ -n "$_promptly_cmd" ]; then printf '\e]133;D;%s\a' "$ret"; fi
    _promptly_cmd=
    printf '\e]7;file://%s%s\a' "$HOSTNAME" "${PWD// /%20}"
    printf '\e]133;A\a'
  }
  _promptly_debug() {
    [ -n "$COMP_LINE" ] && return
    [ "$BASH_COMMAND" = "_promptly_prompt" ] && return
    if [ -z "$_promptly_cmd" ]; then _promptly_cmd=1; printf '\e]133;C\a'; fi
  }
  PROMPT_COMMAND="_promptly_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  trap '_promptly_debug' DEBUG
;; esac
"#;

const FISH: &str = r#"# Promptly shell integration for fish.
status is-interactive; or exit
function __promptly_prompt --on-event fish_prompt
    if set -q __promptly_cmd
        printf '\e]133;D;%s\a' $__promptly_status
        set -e __promptly_cmd
    end
    printf '\e]7;file://%s%s\a' (hostname) (string replace -a ' ' '%20' $PWD)
    printf '\e]133;A\a'
end
function __promptly_preexec --on-event fish_preexec
    set -g __promptly_cmd 1
    printf '\e]133;C\a'
end
function __promptly_postexec --on-event fish_postexec
    set -g __promptly_status $status
end
"#;

fn root() -> PathBuf {
    promptly_core::paths::data_dir().join("shell-integration")
}

/// Write the scripts (idempotent). Called once at startup.
pub fn install() -> std::io::Result<()> {
    let r = root();
    std::fs::create_dir_all(r.join("zsh"))?;
    std::fs::create_dir_all(r.join("fish/vendor_conf.d"))?;
    std::fs::write(r.join("zsh/.zshenv"), ZSHENV)?;
    std::fs::write(r.join("bash-rc"), BASHRC)?;
    std::fs::write(r.join("fish/vendor_conf.d/promptly.fish"), FISH)?;
    Ok(())
}

/// Program, args and env for an interactive login shell with integration.
pub fn shell_command(
    shell: &str,
    integrate: bool,
    env: &mut HashMap<String, String>,
) -> (String, Vec<String>) {
    let name = Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let r = root();
    if integrate {
        match name {
            "zsh" => {
                if let Ok(orig) = std::env::var("ZDOTDIR") {
                    env.insert("PROMPTLY_ORIG_ZDOTDIR".into(), orig);
                }
                env.insert("ZDOTDIR".into(), r.join("zsh").to_string_lossy().into());
                return (shell.into(), vec!["-l".into()]);
            }
            "bash" => {
                let rc = r.join("bash-rc").to_string_lossy().to_string();
                return (shell.into(), vec!["--rcfile".into(), rc, "-i".into()]);
            }
            "fish" => {
                let mut dirs = r.join("fish").to_string_lossy().to_string();
                let existing = std::env::var("XDG_DATA_DIRS")
                    .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
                dirs.push(':');
                dirs.push_str(&existing);
                env.insert("XDG_DATA_DIRS".into(), dirs);
                return (shell.into(), vec!["-l".into()]);
            }
            _ => {}
        }
    }
    (shell.into(), vec!["-l".into()])
}

pub fn user_shell(configured: Option<&str>) -> String {
    configured
        .map(str::to_owned)
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".into())
}
