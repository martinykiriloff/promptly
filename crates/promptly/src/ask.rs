//! "Ask Claude for a command" in shell panes: runs `claude -p` in the
//! background and shows the suggestion as a card above the composer.

use egui::{CornerRadius, FontId, RichText, Stroke, vec2};
use parking_lot::Mutex;
use promptly_core::nl_command::{self, Risk, ShellContext, Suggestion};
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::pty::PaneId;
use crate::theme::{self, tokens as t};
use crate::ui_kit::{self as kit, Icon};

/// Give up on a reply after this long.
const TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone)]
pub enum Status {
    Thinking,
    Ready(Suggestion),
    Failed(String),
}

pub struct Ask {
    pub pane: PaneId,
    pub request: String,
    pub status: Arc<Mutex<Status>>,
    started: Instant,
    child: Arc<Mutex<Option<Child>>>,
}

/// How to launch `claude -p` for a pane.
pub struct Launch {
    /// The user's login shell (so PATH matches their terminal).
    pub shell: String,
    pub binary: String,
    pub model: String,
    pub cwd: PathBuf,
    /// `CLAUDE_CONFIG_DIR` for the pane's account, if not the default.
    pub config_dir: Option<String>,
}

impl Ask {
    pub fn start(
        pane: PaneId,
        request: String,
        context: ShellContext,
        launch: Launch,
        ctx: egui::Context,
    ) -> Self {
        let status = Arc::new(Mutex::new(Status::Thinking));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let system = nl_command::system_prompt(&context);
        let prompt = nl_command::user_prompt(&request, &context);
        let args = nl_command::claude_args(&launch.model, &system, &prompt);
        let argv = crate::app::login_shell_exec(&launch.shell, launch.binary.clone(), args);
        let mut cmd = Command::new(&launch.shell);
        cmd.args(argv)
            .current_dir(&launch.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match &launch.config_dir {
            Some(d) => cmd.env(promptly_core::accounts::CONFIG_DIR_ENV, d),
            None => cmd.env_remove(promptly_core::accounts::CONFIG_DIR_ENV),
        };
        {
            let status = status.clone();
            let child = child.clone();
            let ctx = ctx.clone();
            let binary = launch.binary.clone();
            std::thread::Builder::new()
                .name("ask claude".into())
                .spawn(move || {
                    *status.lock() = match run(cmd, &child, &binary) {
                        Ok(s) => Status::Ready(s),
                        Err(e) => Status::Failed(e),
                    };
                    ctx.request_repaint();
                })
                .ok();
        }
        Self {
            pane,
            request,
            status,
            started: Instant::now(),
            child,
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    /// The command, once there is one to run.
    pub fn ready(&self) -> Option<Suggestion> {
        match self.status() {
            Status::Ready(s) if !s.command.is_empty() => Some(s),
            _ => None,
        }
    }
}

impl Drop for Ask {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn run(mut cmd: Command, slot: &Mutex<Option<Child>>, binary: &str) -> Result<Suggestion, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start your shell: {e}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    *slot.lock() = Some(child);
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    // Watchdog: kill the process if it outlives the timeout.
    let started = Instant::now();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    loop {
        if reader.is_finished() {
            break;
        }
        if started.elapsed() > TIMEOUT {
            if let Some(c) = slot.lock().as_mut() {
                let _ = c.kill();
            }
            return Err("Claude took too long to answer. Try again.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let exit = slot.lock().as_mut().and_then(|c| c.wait().ok());
    match nl_command::parse_print_output(&out) {
        Ok(s) => Ok(s),
        Err(e) if out.trim().is_empty() => {
            let not_found = err.contains("command not found") || err.contains("not found");
            if not_found {
                Err(format!(
                    "`{binary}` wasn't found. Install Claude Code or set claude.binary in the config."
                ))
            } else {
                let line = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                Err(match (line.is_empty(), exit) {
                    (false, _) => line.chars().take(200).collect(),
                    (true, Some(code)) => format!("claude exited ({code}) without a reply"),
                    (true, None) => e,
                })
            }
        }
        Err(e) => Err(e),
    }
}

/// Commands, builtins, aliases and functions the user's shell knows, plus
/// the executables on its PATH. Runs the login shell once (a few hundred ms).
pub fn load_known_commands(shell: &str) -> HashSet<String> {
    let name = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let script = match name {
        "zsh" => {
            r#"print -r -- "__PATH__=$PATH"; print -rl -- ${(k)aliases} ${(k)functions} ${(k)builtins} ${(k)reswords}"#
        }
        "bash" => r#"echo "__PATH__=$PATH"; compgen -abk -A function"#,
        "fish" => r#"echo "__PATH__="(string join : $PATH); functions -a -n; builtin -n"#,
        _ => r#"echo "__PATH__=$PATH""#,
    };
    let mut args = vec!["-l"];
    if name != "fish" {
        args.push("-i");
    }
    args.extend(["-c", script]);
    let out = Command::new(shell)
        .args(&args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut path = std::env::var("PATH").unwrap_or_default();
    let mut known: HashSet<String> = HashSet::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("__PATH__=") {
            path = format!("{p}:{path}");
        } else {
            let w = line.trim();
            if !w.is_empty() && !w.contains(char::is_whitespace) {
                known.insert(w.to_string());
            }
        }
    }
    known.extend(nl_command::path_executables(&path));
    known
}

/// What the user did on the card.
pub enum CardAction {
    Run(String),
    Edit(String),
    Copy(String),
    Retry,
    Dismiss,
}

/// The suggestion card above the composer.
pub fn card(ui: &mut egui::Ui, ask: &Ask) -> Option<CardAction> {
    let mut action = None;
    let status = ask.status();
    egui::Frame::new()
        .fill(t::BG_ELEVATED)
        .stroke(Stroke::new(1.0, t::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                kit::paint_icon(ui, r, Icon::Sparkle, t::ACCENT_HOVER);
                let w = ui.available_width() - 40.0;
                let g = kit::elide(ui, &ask.request, FontId::proportional(12.5), t::TEXT_2, w);
                ui.label(g);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button(ui, Icon::Close, "Dismiss (Esc)", false).clicked() {
                        action = Some(CardAction::Dismiss);
                    }
                });
            });
            ui.add_space(6.0);
            match &status {
                Status::Thinking => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(t::TEXT_2));
                        ui.label(
                            RichText::new(format!(
                                "Writing a command…  {}s",
                                ask.started.elapsed().as_secs()
                            ))
                            .size(13.0)
                            .color(t::TEXT_2),
                        );
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
                Status::Failed(e) => {
                    ui.label(RichText::new(e).size(12.5).color(theme::RED));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if kit::secondary_button(ui, Some(Icon::Refresh), "Try again").clicked() {
                            action = Some(CardAction::Retry);
                        }
                    });
                }
                Status::Ready(s) if s.command.is_empty() => {
                    ui.label(RichText::new(&s.explanation).size(13.0).color(t::TEXT_1));
                }
                Status::Ready(s) => {
                    egui::Frame::new()
                        .fill(t::BG_INPUT)
                        .stroke(Stroke::new(1.0, t::BORDER))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&s.command)
                                        .font(FontId::monospace(13.5))
                                        .color(t::TEXT),
                                )
                                .wrap()
                                .selectable(true),
                            );
                        });
                    ui.add_space(8.0);
                    if !s.explanation.is_empty() {
                        ui.label(RichText::new(&s.explanation).size(12.5).color(t::TEXT_2));
                    }
                    if !s.note.is_empty() {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!("Then: {}", s.note))
                                .size(12.0)
                                .color(t::TEXT_3),
                        );
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        match s.risk {
                            Risk::Danger => {
                                kit::pill(ui, "Destructive — read it before running", theme::RED);
                            }
                            Risk::Caution => {
                                kit::pill(ui, "Changes your system", theme::AMBER);
                            }
                            Risk::Safe => {
                                kit::pill(ui, "Read-only", theme::GREEN);
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if s.risk == Risk::Danger {
                                "Run anyway"
                            } else {
                                "Run"
                            };
                            let sc = (s.risk != Risk::Danger).then_some("↵");
                            if kit::primary_button(ui, Some(Icon::Terminal), label, sc, false)
                                .clicked()
                            {
                                action = Some(CardAction::Run(s.command.clone()));
                            }
                            if kit::secondary_button(ui, None, "Edit  ⇥")
                                .on_hover_text("Put the command in the composer to change it (Tab)")
                                .clicked()
                            {
                                action = Some(CardAction::Edit(s.command.clone()));
                            }
                            if kit::secondary_button(ui, None, "Copy").clicked() {
                                action = Some(CardAction::Copy(s.command.clone()));
                            }
                        });
                    });
                }
            }
        });
    action
}
