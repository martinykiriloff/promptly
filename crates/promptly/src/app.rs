//! The main window: sessions + attention queue on the left, terminal panes
//! (with docked composer) in the middle, review pane on the right.

use egui::{Color32, FontId, Key, KeyboardShortcut, Modifiers, RichText};
use parking_lot::Mutex;
use promptly_core::config::{self, Config};
use promptly_core::git;
use promptly_core::hooks::{self, InjectedSettings};
use promptly_core::index::{SessionIndex, SessionRecord};
use promptly_core::ipc::{self, ControlRequest, Incoming, Server};
use promptly_core::sanitize;
use promptly_core::state::SessionState;
use promptly_core::util;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::composer::{Composer, ComposerAction};
use crate::events::AppEvent;
use crate::pty::{Pane, PaneId, ShellMark, SpawnSpec};
use crate::review::{ReviewAction, ReviewState};
use crate::session::{
    ClaudeLink, Core, LinkCtx, PaneKind, SavedLayout, SavedPane, SessionMeta, SharedCore,
};
use crate::term_view::{self, SearchState, ViewOptions, ViewState};
use crate::ui_kit::{self as kit, Icon};
use crate::{charts, shell_integration, theme};

struct PaneEntry {
    pane: Pane,
    view: ViewState,
    _link: Option<ClaudeLink>,
}

struct Tab {
    panes: Vec<PaneId>,
    vertical: bool,
}

#[derive(Default)]
pub struct NewSession {
    pub cwd: Option<PathBuf>,
    pub claude: bool,
    pub resume: Option<String>,
    pub worktree: Option<String>,
    pub prompt: Option<String>,
    pub split: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Palette,
    NewClaude,
    NewClaudeWorktree,
    NewShell,
    SplitRight,
    SplitDown,
    ClosePane,
    NextAttention,
    ToggleReview,
    ToggleComposer,
    ToggleGrid,
    History,
    FanOut,
    Search,
    Settings,
    EventLog,
    CopyDiagnostics,
    NextPane,
    PrevPane,
    FontUp,
    FontDown,
    ReloadConfig,
    RemoveWorktree,
    TranscriptView,
    Usage,
    CheckUpdates,
    InstallUpdate,
}

const ACTIONS: &[(Action, &str, &str, &str)] = &[
    (
        Action::Palette,
        "palette",
        "Command palette",
        "primary+shift+p",
    ),
    (
        Action::NewClaude,
        "new_claude",
        "New Claude session",
        "primary+n",
    ),
    (
        Action::NewClaudeWorktree,
        "new_claude_worktree",
        "New Claude session in a git worktree",
        "primary+shift+n",
    ),
    (Action::NewShell, "new_shell", "New shell tab", "primary+t"),
    (
        Action::SplitRight,
        "split_right",
        "Split right",
        "primary+d",
    ),
    (
        Action::SplitDown,
        "split_down",
        "Split down",
        "primary+shift+d",
    ),
    (Action::ClosePane, "close_pane", "Close pane", "primary+w"),
    (
        Action::NextAttention,
        "next_attention",
        "Jump to next session that needs you",
        "primary+j",
    ),
    (
        Action::ToggleReview,
        "toggle_review",
        "Toggle review pane",
        "primary+e",
    ),
    (
        Action::ToggleComposer,
        "toggle_composer",
        "Toggle composer",
        "primary+l",
    ),
    (
        Action::ToggleGrid,
        "toggle_grid",
        "Toggle session grid",
        "primary+g",
    ),
    (
        Action::History,
        "history",
        "Search and resume past sessions",
        "primary+r",
    ),
    (
        Action::FanOut,
        "fan_out",
        "Fan out: one session per task",
        "primary+shift+f",
    ),
    (Action::Search, "search", "Search scrollback", "primary+f"),
    (Action::Settings, "settings", "Settings", "primary+comma"),
    (
        Action::EventLog,
        "event_log",
        "Hook event log",
        "primary+shift+e",
    ),
    (
        Action::CopyDiagnostics,
        "copy_diagnostics",
        "Copy diagnostics",
        "",
    ),
    (
        Action::NextPane,
        "next_pane",
        "Next session",
        "primary+closebracket",
    ),
    (
        Action::PrevPane,
        "prev_pane",
        "Previous session",
        "primary+openbracket",
    ),
    (
        Action::FontUp,
        "font_up",
        "Increase font size",
        "primary+equals",
    ),
    (
        Action::FontDown,
        "font_down",
        "Decrease font size",
        "primary+minus",
    ),
    (Action::ReloadConfig, "reload_config", "Reload config", ""),
    (
        Action::RemoveWorktree,
        "remove_worktree",
        "Remove this session's worktree",
        "",
    ),
    (
        Action::TranscriptView,
        "transcript_view",
        "Show session transcript",
        "primary+shift+t",
    ),
    (Action::Usage, "usage", "Usage dashboard", "primary+u"),
    (
        Action::CheckUpdates,
        "check_updates",
        "Check for updates",
        "",
    ),
    (
        Action::InstallUpdate,
        "install_update",
        "Install available update",
        "",
    ),
];

struct Palette {
    query: String,
    sel: usize,
}

struct HistoryPicker {
    query: String,
    results: Vec<SessionRecord>,
    sel: usize,
    dirty: bool,
}

struct FanOut {
    tasks: String,
    cwd: String,
    worktrees: bool,
}

pub struct App {
    cfg: Config,
    cfg_error: Option<String>,
    core: SharedCore,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    ctx: egui::Context,
    panes: BTreeMap<PaneId, PaneEntry>,
    tabs: Vec<Tab>,
    active_tab: usize,
    active: Option<PaneId>,
    next_id: PaneId,
    font_size: f32,
    composer: Composer,
    composer_open: bool,
    review: ReviewState,
    review_open: bool,
    palette: Option<Palette>,
    history: Option<HistoryPicker>,
    fanout: Option<FanOut>,
    settings_open: bool,
    log_open: bool,
    transcript_open: bool,
    grid_mode: bool,
    usage_mode: bool,
    reduce_motion: bool,
    update: Arc<Mutex<UpdateState>>,
    paste_review: Option<(PaneId, String)>,
    restore: Option<SavedLayout>,
    index: Arc<Mutex<Option<SessionIndex>>>,
    _control: Option<Server>,
    control_path: Option<PathBuf>,
    ctl_path: Option<PathBuf>,
    hooks_locked: bool,
    toast: Option<(String, Instant)>,
    focus_terminal: bool,
    last_poll: Instant,
    shortcuts: Vec<(Action, KeyboardShortcut)>,
    option_as_meta: bool,
    high_contrast: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let (cfg, cfg_error) = Config::load();
        crate::fonts::install(&ctx);
        theme::apply_chrome(&ctx, false);
        let _ = shell_integration::install();
        let (tx, rx) = mpsc::channel();
        let core: SharedCore = Arc::new(Mutex::new(Core::new()));
        {
            let mut c = core.lock();
            c.notifications_enabled = cfg.notifications.enabled;
            c.policy.notify_on_finish = cfg.notifications.on_finish;
            c.policy.max_per_minute = cfg.notifications.max_per_minute;
        }

        // Control socket for `promptly-ctl new-session|notify|list|focus`.
        let control_path = ipc::control_socket_path().ok();
        let control = control_path.as_ref().and_then(|p| {
            let tx = tx.clone();
            let ctx = ctx.clone();
            Server::spawn(p.clone(), None, move |m| {
                if let Incoming::Control(req, resp) = m {
                    let _ = tx.send(AppEvent::Control(req, resp));
                    ctx.request_repaint();
                }
            })
            .ok()
        });

        // Session index: refresh in the background now and every two minutes.
        let index: Arc<Mutex<Option<SessionIndex>>> = Arc::new(Mutex::new(None));
        {
            let index = index.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("session index".into())
                .spawn(move || {
                    let db = promptly_core::paths::data_dir().join("index.sqlite");
                    let Ok(mut idx) = SessionIndex::open(&db) else {
                        return;
                    };
                    let _ = idx.refresh(&promptly_core::paths::claude_projects_dir());
                    *index.lock() = Some(idx);
                    let _ = tx.send(AppEvent::IndexRefreshed);
                    ctx.request_repaint();
                    loop {
                        std::thread::sleep(Duration::from_secs(120));
                        if let Some(idx) = index.lock().as_mut() {
                            let _ = idx.refresh(&promptly_core::paths::claude_projects_dir());
                        }
                        let _ = tx.send(AppEvent::IndexRefreshed);
                    }
                })
                .ok();
        }

        // Today's tokens across all Claude sessions, rescanned every 30 s.
        {
            let core = core.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("usage scan".into())
                .spawn(move || {
                    loop {
                        let d = promptly_core::usage::scan_today(
                            &promptly_core::paths::claude_projects_dir(),
                            promptly_core::usage::now_secs(),
                        );
                        core.lock().daily = Some(d);
                        ctx.request_repaint();
                        std::thread::sleep(Duration::from_secs(30));
                    }
                })
                .ok();
        }
        let ctl_path = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("promptly-ctl")))
            .filter(|p| p.exists());
        let restore = SavedLayout::load().filter(|l| l.tabs.iter().any(|t| !t.is_empty()));
        let shortcuts = build_shortcuts(&cfg);
        let mut app = Self {
            font_size: cfg.font_size,
            cfg,
            cfg_error,
            core,
            tx,
            rx,
            ctx,
            panes: BTreeMap::new(),
            tabs: vec![],
            active_tab: 0,
            active: None,
            next_id: 1,
            composer: Composer::with_history(Composer::load_history()),
            composer_open: true,
            review: ReviewState::default(),
            review_open: false,
            palette: None,
            history: None,
            fanout: None,
            settings_open: false,
            log_open: false,
            transcript_open: false,
            grid_mode: false,
            usage_mode: false,
            reduce_motion: false,
            update: Arc::new(Mutex::new(UpdateState::Idle)),
            paste_review: None,
            restore,
            index,
            _control: control,
            control_path,
            hooks_locked: hooks::managed_hooks_disabled(),
            ctl_path,
            toast: None,
            focus_terminal: true,
            last_poll: Instant::now() - Duration::from_secs(60),
            shortcuts,
            option_as_meta: false,
            high_contrast: false,
        };
        if app.ctl_path.is_none() {
            app.toast("promptly-ctl not found next to the app: running in transcript-only mode");
        }
        if let Some(l) = &app.restore {
            app.review_open = l.review_open;
            app.composer_open = l.composer_open;
        }
        app.spawn(NewSession::default());
        if app.cfg.updates.check {
            let state = app.update.clone();
            let ctx = app.ctx.clone();
            std::thread::Builder::new()
                .name("update check".into())
                .spawn(move || {
                    std::thread::sleep(Duration::from_secs(8));
                    loop {
                        check_for_update(&state, &ctx, false);
                        std::thread::sleep(Duration::from_secs(6 * 3600));
                    }
                })
                .ok();
        }
        // Open a surface on launch; used for screenshots / golden-image tests.
        match std::env::var("PROMPTLY_DEBUG_OPEN").as_deref() {
            Ok("palette") => app.run(Action::Palette),
            Ok("history") => app.run(Action::History),
            Ok("grid") => app.run(Action::ToggleGrid),
            Ok("fanout") => app.run(Action::FanOut),
            Ok("review") => app.review_open = true,
            Ok("usage") => app.run(Action::Usage),
            _ => {}
        }
        app
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn font(&self) -> FontId {
        FontId::monospace(self.font_size)
    }

    // ------------------------------------------------------------ sessions

    fn default_cwd(&self) -> PathBuf {
        self.active
            .and_then(|id| self.core.lock().sessions.get(&id).map(|m| m.cwd.clone()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(promptly_core::paths::home)
    }

    pub fn spawn(&mut self, req: NewSession) -> Option<PaneId> {
        match self.try_spawn(req) {
            Ok(id) => Some(id),
            Err(e) => {
                self.toast(format!("Could not start session: {e}"));
                None
            }
        }
    }

    fn try_spawn(&mut self, req: NewSession) -> anyhow::Result<PaneId> {
        let id = self.next_id;
        self.next_id += 1;
        let mut cwd = req.cwd.clone().unwrap_or_else(|| self.default_cwd());
        let profile = self.cfg.profile_for(&cwd).cloned();
        let mut env: HashMap<String, String> = profile
            .as_ref()
            .map(|p| p.env.clone().into_iter().collect())
            .unwrap_or_default();
        if let Some(c) = &self.control_path {
            env.insert(
                promptly_core::env::CONTROL.into(),
                c.to_string_lossy().into(),
            );
        }
        env.insert(promptly_core::env::SESSION.into(), id.to_string());
        if let Some(ctl) = &self.ctl_path
            && let Some(dir) = ctl.parent()
        {
            let path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{path}", dir.display()));
        }
        let shell = shell_integration::user_shell(self.cfg.shell.as_deref());
        let mut worktree = None;
        let mut link = None;

        let (kind, program, args) = if req.claude {
            let wt_name = req.worktree.clone().or_else(|| {
                profile
                    .as_ref()
                    .filter(|p| p.worktree_per_session && req.resume.is_none())
                    .map(|_| {
                        req.prompt
                            .as_deref()
                            .map(|p| sanitize::one_line(p, 40))
                            .unwrap_or_else(|| format!("session-{id}"))
                    })
            });
            if let Some(name) = wt_name {
                match git::create_worktree(&cwd, &name) {
                    Ok(wt) => {
                        cwd = wt.path.clone();
                        worktree = Some(wt);
                    }
                    Err(e) => self.toast(format!(
                        "Worktree not created ({e}); using {}",
                        cwd.display()
                    )),
                }
            }
            let session_id = req.resume.clone().unwrap_or_else(util::new_uuid);
            let mut args = config::claude_args(&self.cfg, profile.as_ref());
            if let Some(r) = &req.resume {
                args.extend(["--resume".into(), r.clone()]);
            } else {
                args.extend(["--session-id".into(), session_id.clone()]);
            }
            let inject =
                self.cfg.claude.inject_hooks && !self.hooks_locked && self.ctl_path.is_some();
            if inject {
                let rt = promptly_core::paths::runtime_dir()?;
                let token = util::random_token();
                let socket = rt.join(format!("s-{id}.sock"));
                let settings_path = rt.join(format!("settings-{id}.json"));
                InjectedSettings::build(
                    self.ctl_path.as_ref().unwrap(),
                    self.cfg.claude.wrap_statusline,
                )
                .write_to(&settings_path)?;
                args.extend(["--settings".into(), settings_path.to_string_lossy().into()]);
                env.insert(
                    promptly_core::env::SOCKET.into(),
                    socket.to_string_lossy().into(),
                );
                env.insert(promptly_core::env::TOKEN.into(), token.clone());
                if self.cfg.claude.wrap_statusline
                    && let Some(cmd) = hooks::user_statusline_command(&cwd)
                {
                    env.insert(promptly_core::env::USER_STATUSLINE.into(), cmd);
                }
                link = Some((socket, token, settings_path));
            }
            if let Some(p) = &req.prompt {
                args.push(p.clone());
            }
            // Run through the user's login shell so PATH (nvm, ~/.local/bin)
            // matches what they get in their own terminal.
            let bin = self.cfg.claude.binary.clone();
            let sh_name = Path::new(&shell)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let sh_args = if sh_name == "fish" {
                let cmd = std::iter::once(bin)
                    .chain(args)
                    .map(|a| hooks::shell_quote(&a))
                    .collect::<Vec<_>>()
                    .join(" ");
                vec!["-l".into(), "-c".into(), format!("exec {cmd}")]
            } else {
                let mut v = vec![
                    "-l".into(),
                    "-i".into(),
                    "-c".into(),
                    r#"exec "$0" "$@""#.into(),
                    bin,
                ];
                v.extend(args);
                v
            };
            (PaneKind::Claude { session_id }, shell.clone(), sh_args)
        } else {
            let (program, args) =
                shell_integration::shell_command(&shell, self.cfg.shell_integration, &mut env);
            (PaneKind::Shell, program, args)
        };

        let pane = Pane::spawn(
            id,
            SpawnSpec {
                program,
                args,
                cwd: Some(cwd.clone()),
                env,
                scrollback: self.cfg.scrollback_lines,
            },
            self.tx.clone(),
            self.ctx.clone(),
        )?;
        if kind == PaneKind::Shell
            && let Some(cmd) = profile.as_ref().and_then(|p| p.startup_command.clone())
        {
            pane.write(format!("{cmd}\r").into_bytes());
        }
        let transcript = match &kind {
            PaneKind::Claude { session_id } => {
                Some(promptly_core::paths::transcript_path_for(&cwd, session_id))
            }
            PaneKind::Shell => None,
        };
        {
            let mut meta = SessionMeta::new(kind.clone(), cwd.clone());
            meta.worktree = worktree;
            meta.hooks_injected = link.is_some();
            meta.transcript_path = transcript.clone();
            self.core.lock().sessions.insert(id, meta);
        }
        let claude_link = match (link, transcript) {
            (Some((socket, token, settings)), Some(t)) => Some(ClaudeLink::start(
                id,
                socket,
                token,
                settings,
                t,
                LinkCtx {
                    core: self.core.clone(),
                    tx: self.tx.clone(),
                    ctx: self.ctx.clone(),
                },
            )?),
            (None, Some(t)) => {
                // Transcript-only mode: still tail the transcript for state.
                let rt = promptly_core::paths::runtime_dir()?;
                Some(ClaudeLink::start(
                    id,
                    rt.join(format!("s-{id}.sock")),
                    util::random_token(),
                    rt.join(format!("settings-{id}.json")),
                    t,
                    LinkCtx {
                        core: self.core.clone(),
                        tx: self.tx.clone(),
                        ctx: self.ctx.clone(),
                    },
                )?)
            }
            _ => None,
        };
        self.panes.insert(
            id,
            PaneEntry {
                pane,
                view: ViewState::default(),
                _link: claude_link,
            },
        );
        match req.split {
            Some(vertical) if !self.tabs.is_empty() => {
                let t = &mut self.tabs[self.active_tab];
                t.vertical = vertical;
                t.panes.push(id);
            }
            _ => {
                self.tabs.push(Tab {
                    panes: vec![id],
                    vertical: false,
                });
                self.active_tab = self.tabs.len() - 1;
            }
        }
        self.focus(id);
        self.refresh_branch(id);
        Ok(id)
    }

    fn focus(&mut self, id: PaneId) {
        if let Some(t) = self.tabs.iter().position(|t| t.panes.contains(&id)) {
            self.active_tab = t;
            self.active = Some(id);
            self.focus_terminal = true;
            self.grid_mode = false;
            self.usage_mode = false;
        }
    }

    fn close(&mut self, id: PaneId) {
        self.panes.remove(&id);
        let meta = {
            let mut c = self.core.lock();
            c.attention.remove(id);
            c.sessions.remove(&id)
        };
        if let Some(wt) = meta.and_then(|m| m.worktree) {
            match git::is_dirty(&wt.path) {
                Some(false) => self.toast(format!(
                    "Worktree kept at {} (clean; remove it from the palette)",
                    wt.path.display()
                )),
                _ => self.toast(format!(
                    "Worktree {} has uncommitted changes and was kept",
                    wt.path.display()
                )),
            }
        }
        for t in &mut self.tabs {
            t.panes.retain(|p| *p != id);
        }
        self.tabs.retain(|t| !t.panes.is_empty());
        if self.tabs.is_empty() {
            self.active = None;
            self.active_tab = 0;
            return;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        if self.active == Some(id) {
            self.active = self.tabs[self.active_tab].panes.last().copied();
            self.focus_terminal = true;
        }
    }

    fn refresh_branch(&self, id: PaneId) {
        let core = self.core.clone();
        let Some(cwd) = core.lock().sessions.get(&id).map(|m| m.cwd.clone()) else {
            return;
        };
        std::thread::spawn(move || {
            let b = git::current_branch(&cwd);
            if let Some(m) = core.lock().sessions.get_mut(&id) {
                m.branch = b;
            }
        });
    }

    fn ordered_panes(&self) -> Vec<PaneId> {
        self.tabs
            .iter()
            .flat_map(|t| t.panes.iter().copied())
            .collect()
    }

    fn send_prompt(&mut self, id: PaneId, text: &str) {
        let Some(e) = self.panes.get(&id) else { return };
        let bracketed = e
            .pane
            .term
            .lock()
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        e.pane
            .write(sanitize::encode_paste(text.trim_end(), bracketed));
        // Give the TUI a moment to ingest the paste before submitting.
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            let _ = tx.send(AppEvent::Submit(id));
            ctx.request_repaint();
        });
        self.ctx.request_repaint_after(Duration::from_millis(80));
        if let Some(m) = self.core.lock().sessions.get_mut(&id) {
            m.tracker.acknowledge();
        }
    }

    // ------------------------------------------------------------ events

    fn process_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Term(id, ev) => self.on_term_event(id, ev),
                AppEvent::Shell(id, mark) => {
                    let mut c = self.core.lock();
                    if let Some(m) = c.sessions.get_mut(&id) {
                        match mark {
                            ShellMark::Cwd(p) => m.cwd = p,
                            ShellMark::CommandExecuted => m.command_running = true,
                            ShellMark::CommandFinished(code) => {
                                m.command_running = false;
                                m.last_exit = code;
                                m.change_seq += 1;
                            }
                            ShellMark::PromptStart | ShellMark::CommandStart => {}
                        }
                    }
                }
                AppEvent::Control(req, resp) => self.on_control(req, resp),
                AppEvent::FocusSession(id) => {
                    self.focus(id);
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                AppEvent::Submit(id) => {
                    if let Some(e) = self.panes.get(&id) {
                        e.pane.write(b"\r".as_slice());
                    }
                }
                AppEvent::IndexRefreshed => {
                    if let Some(h) = self.history.as_mut() {
                        h.dirty = true;
                    }
                }
            }
        }
    }

    fn on_term_event(&mut self, id: PaneId, ev: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event as E;
        let note = {
            let mut c = self.core.lock();
            let Some(m) = c.sessions.get_mut(&id) else {
                return;
            };
            match ev {
                E::Title(t) => {
                    m.title = sanitize::title(&t);
                    None
                }
                E::ResetTitle => {
                    m.title.clear();
                    None
                }
                E::Bell => {
                    let tr = m.tracker.on_bell();
                    c.apply(id, tr)
                }
                E::ClipboardStore(_, text) => {
                    match m.clipboard_allowed {
                        Some(true) => self.ctx.copy_text(text),
                        Some(false) => {}
                        None => m.pending_clipboard = Some(text),
                    }
                    None
                }
                E::ChildExit(status) => {
                    let code = status.code();
                    m.last_exit = code;
                    let tr = m.tracker.on_exit(code);
                    c.log(id, "pty", format!("child exited: {status}"));
                    c.apply(id, tr)
                }
                _ => None,
            }
        };
        if let Some((t, b)) = note {
            crate::notify::send(t, b, Some(id), self.tx.clone(), self.ctx.clone());
        }
    }

    fn on_control(&mut self, req: ControlRequest, resp: promptly_core::ipc::Responder) {
        use serde_json::json;
        match req {
            ControlRequest::NewSession {
                cwd,
                claude,
                worktree,
                prompt,
            } => {
                let id = self.spawn(NewSession {
                    cwd,
                    claude,
                    worktree,
                    prompt,
                    ..Default::default()
                });
                resp.reply(match id {
                    Some(id) => json!({"ok": true, "session": id}),
                    None => json!({"ok": false, "error": self.toast.as_ref().map(|t| t.0.clone())}),
                });
            }
            ControlRequest::Notify { title, body } => {
                crate::notify::send(
                    sanitize::one_line(&title, 80),
                    sanitize::one_line(&body, 300),
                    None,
                    self.tx.clone(),
                    self.ctx.clone(),
                );
                resp.reply(json!({"ok": true}));
            }
            ControlRequest::Action { name } => match ACTIONS.iter().find(|a| a.1 == name) {
                Some((a, ..)) => {
                    self.run(*a);
                    resp.reply(json!({"ok": true}));
                }
                None => {
                    let names: Vec<_> = ACTIONS.iter().map(|a| a.1).collect();
                    resp.reply(json!({"ok": false, "error": "unknown action", "actions": names}));
                }
            },
            ControlRequest::ListSessions => {
                let c = self.core.lock();
                let list: Vec<_> = c
                    .sessions
                    .iter()
                    .map(|(id, m)| {
                        json!({
                            "session": id,
                            "kind": if m.is_claude() { "claude" } else { "shell" },
                            "claude_session_id": match &m.kind { PaneKind::Claude { session_id } => Some(session_id.clone()), _ => None },
                            "state": m.state().label(),
                            "cwd": m.cwd,
                            "branch": m.branch,
                            "title": m.display_name(),
                            "cost_usd": m.status.cost_usd,
                            "context_percent": m.context_percent(),
                        })
                    })
                    .collect();
                resp.reply(json!({"ok": true, "sessions": list}));
            }
            ControlRequest::Focus { session } => {
                let target = session
                    .parse::<PaneId>()
                    .ok()
                    .filter(|id| self.panes.contains_key(id))
                    .or_else(|| {
                        self.core
                            .lock()
                            .sessions
                            .iter()
                            .find_map(|(id, m)| match &m.kind {
                                PaneKind::Claude { session_id } if *session_id == session => {
                                    Some(*id)
                                }
                                _ => None,
                            })
                    });
                match target {
                    Some(id) => {
                        self.focus(id);
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        resp.reply(json!({"ok": true}));
                    }
                    None => resp.reply(json!({"ok": false, "error": "no such session"})),
                }
            }
        }
    }

    // ------------------------------------------------------------ actions

    fn run(&mut self, a: Action) {
        match a {
            Action::Palette => {
                self.palette = Some(Palette {
                    query: String::new(),
                    sel: 0,
                })
            }
            Action::NewClaude => {
                self.spawn(NewSession {
                    claude: true,
                    ..Default::default()
                });
            }
            Action::NewClaudeWorktree => {
                let name = format!("session-{}", self.next_id);
                self.spawn(NewSession {
                    claude: true,
                    worktree: Some(name),
                    ..Default::default()
                });
            }
            Action::NewShell => {
                self.spawn(NewSession::default());
            }
            Action::SplitRight => {
                self.spawn(NewSession {
                    split: Some(false),
                    ..Default::default()
                });
            }
            Action::SplitDown => {
                self.spawn(NewSession {
                    split: Some(true),
                    ..Default::default()
                });
            }
            Action::ClosePane => {
                if let Some(id) = self.active {
                    self.close(id);
                }
            }
            Action::NextAttention => {
                let next = self
                    .core
                    .lock()
                    .attention
                    .ranked()
                    .first()
                    .map(|i| i.session);
                match next {
                    Some(id) => self.focus(id),
                    None => self.toast("No session needs you"),
                }
            }
            Action::ToggleReview => self.review_open = !self.review_open,
            Action::ToggleComposer => {
                self.composer_open = !self.composer_open;
                self.composer.focus_requested = self.composer_open;
                self.focus_terminal = !self.composer_open;
            }
            Action::ToggleGrid => {
                self.grid_mode = !self.grid_mode;
                self.usage_mode = false;
            }
            Action::History => {
                self.history = Some(HistoryPicker {
                    query: String::new(),
                    results: vec![],
                    sel: 0,
                    dirty: true,
                })
            }
            Action::FanOut => {
                self.fanout = Some(FanOut {
                    tasks: String::new(),
                    cwd: self.default_cwd().to_string_lossy().into(),
                    worktrees: true,
                })
            }
            Action::Search => {
                if let Some(e) = self.active.and_then(|id| self.panes.get_mut(&id)) {
                    e.view.search = Some(SearchState::new());
                }
            }
            Action::Settings => self.settings_open = !self.settings_open,
            Action::EventLog => self.log_open = !self.log_open,
            Action::CopyDiagnostics => {
                let d = self.diagnostics();
                self.ctx.copy_text(d);
                self.toast("Diagnostics copied (secrets redacted)");
            }
            Action::NextPane | Action::PrevPane => {
                let order = self.ordered_panes();
                if let (Some(cur), false) = (self.active, order.is_empty()) {
                    let i = order.iter().position(|p| *p == cur).unwrap_or(0);
                    let n = order.len();
                    let j = if a == Action::NextPane {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    };
                    self.focus(order[j]);
                }
            }
            Action::FontUp => self.font_size = (self.font_size + 1.0).min(32.0),
            Action::FontDown => self.font_size = (self.font_size - 1.0).max(8.0),
            Action::ReloadConfig => {
                let (cfg, err) = Config::load();
                self.shortcuts = build_shortcuts(&cfg);
                self.font_size = cfg.font_size;
                self.cfg = cfg;
                self.cfg_error = err;
                self.toast("Config reloaded");
            }
            Action::RemoveWorktree => {
                let wt = self.active.and_then(|id| {
                    self.core
                        .lock()
                        .sessions
                        .get(&id)
                        .and_then(|m| m.worktree.clone())
                });
                match wt {
                    None => self.toast("This session has no worktree"),
                    Some(wt) => match git::remove_worktree(&wt) {
                        Ok(()) => {
                            self.toast(format!("Removed worktree; branch {} kept", wt.branch));
                            if let Some(id) = self.active {
                                self.close(id);
                            }
                        }
                        Err(e) => self.toast(e),
                    },
                }
            }
            Action::TranscriptView => self.transcript_open = !self.transcript_open,
            Action::CheckUpdates => {
                let (state, ctx) = (self.update.clone(), self.ctx.clone());
                std::thread::spawn(move || check_for_update(&state, &ctx, true));
            }
            Action::InstallUpdate => {
                let release = match &*self.update.lock() {
                    UpdateState::Available(r) => Some(r.clone()),
                    _ => None,
                };
                match release {
                    Some(r) => {
                        let (state, ctx) = (self.update.clone(), self.ctx.clone());
                        std::thread::spawn(move || install_update(&state, &ctx, r));
                    }
                    None => self.toast("No update to install. Use “Check for updates” first."),
                }
            }
            Action::Usage => {
                self.usage_mode = !self.usage_mode;
                self.grid_mode = false;
            }
        }
    }

    fn diagnostics(&self) -> String {
        let c = self.core.lock();
        let mut s = format!(
            "Promptly {}\nos: {} {}\nclaude: {}\nhooks: {}\n\nsessions:\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            util::run_stdout(std::process::Command::new(&self.cfg.claude.binary).arg("--version"))
                .unwrap_or_else(|| "not found".into()),
            if self.hooks_locked {
                "locked by managed settings"
            } else if self.cfg.claude.inject_hooks {
                "injected"
            } else {
                "disabled"
            },
        );
        for (id, m) in &c.sessions {
            s.push_str(&format!(
                "  #{id} {} state={} hooks_seen={} signal={:?}\n",
                if m.is_claude() { "claude" } else { "shell" },
                m.state().label(),
                m.tracker.hooks_seen(),
                m.tracker.last_signal
            ));
        }
        s.push_str("\nrecent events:\n");
        for e in c.log.iter().rev().take(200).rev() {
            s.push_str(&format!(
                "  #{} {} {}\n",
                e.pane,
                e.kind,
                sanitize::redact_secrets(&e.summary)
            ));
        }
        s
    }

    fn save_layout(&self) {
        let c = self.core.lock();
        let tabs = self
            .tabs
            .iter()
            .map(|t| {
                t.panes
                    .iter()
                    .filter_map(|id| c.sessions.get(id))
                    .filter(|m| m.state() != SessionState::Exited)
                    .map(|m| SavedPane {
                        kind: m.kind.clone(),
                        cwd: m.cwd.clone(),
                        title: m.title.clone(),
                        worktree: m
                            .worktree
                            .as_ref()
                            .map(|w| (w.path.clone(), w.branch.clone(), w.repo.clone())),
                    })
                    .collect()
            })
            .collect();
        SavedLayout {
            tabs,
            review_open: self.review_open,
            composer_open: self.composer_open,
        }
        .save();
    }

    fn restore_layout(&mut self, layout: SavedLayout) {
        let first_blank = self.panes.len() == 1 && self.active.is_some();
        let blank = if first_blank { self.active } else { None };
        for tab in layout.tabs {
            for (i, p) in tab.into_iter().enumerate() {
                if !p.cwd.is_dir() {
                    continue;
                }
                let split = if i == 0 { None } else { Some(false) };
                let req = match p.kind {
                    PaneKind::Claude { session_id } => NewSession {
                        cwd: Some(p.cwd),
                        claude: true,
                        resume: Some(session_id),
                        split,
                        ..Default::default()
                    },
                    PaneKind::Shell => NewSession {
                        cwd: Some(p.cwd),
                        split,
                        ..Default::default()
                    },
                };
                let id = self.spawn(req);
                if let (Some(id), Some((path, branch, repo))) = (id, p.worktree)
                    && let Some(m) = self.core.lock().sessions.get_mut(&id)
                {
                    m.worktree = Some(git::Worktree { path, branch, repo });
                }
            }
        }
        if let Some(b) = blank {
            self.close(b);
        }
    }

    // ------------------------------------------------------------ UI

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut fired = vec![];
        ctx.input_mut(|i| {
            for (a, sc) in &self.shortcuts {
                if i.consume_shortcut(sc) {
                    fired.push(*a);
                }
            }
        });
        for a in fired {
            self.run(a);
        }
    }

    /// Formatted shortcut for an action, e.g. "⌘N".
    fn sc(&self, a: Action) -> Option<String> {
        self.shortcuts
            .iter()
            .find(|(x, _)| *x == a)
            .map(|(_, s)| self.ctx.format_shortcut(s))
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let mut focus = None;
        let mut close = None;
        let mut actions = vec![];

        // Title-bar strip: room for the traffic lights, and draggable.
        let top = if cfg!(target_os = "macos") { 34.0 } else { 6.0 };
        let (strip, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), top), egui::Sense::hover());
        window_drag_zone(ui, strip, "sidebar-drag");

        let new_sc = self.sc(Action::NewClaude);
        if kit::primary_button(
            ui,
            Some(Icon::Sparkle),
            "New Claude session",
            new_sc.as_deref(),
            true,
        )
        .clicked()
        {
            actions.push(Action::NewClaude);
        }
        ui.add_space(4.0);
        for (icon, label, action) in [
            (Icon::Terminal, "New shell", Action::NewShell),
            (Icon::Fork, "Fan out tasks", Action::FanOut),
            (Icon::Clock, "Resume a session", Action::History),
        ] {
            let sc = self.sc(action);
            if kit::ghost_button(ui, icon, label, sc.as_deref()).clicked() {
                actions.push(action);
            }
        }
        if let Some(a) = self.update_card(ui) {
            actions.push(a);
        }

        let c = self.core.lock();
        let ranked = c.attention.ranked();
        if !ranked.is_empty() {
            ui.add_space(8.0);
            kit::section_header(ui, "Needs you", Some(ranked.len()), theme::AMBER);
            for item in ranked {
                let Some(m) = c.sessions.get(&item.session) else {
                    continue;
                };
                let detail = item
                    .detail
                    .clone()
                    .unwrap_or_else(|| kit::state_label(item.state).to_string());
                let subtitle = format!(
                    "{} · {}",
                    kit::state_label(item.state),
                    sanitize::one_line(&detail, 80)
                );
                let waited = fmt_dur(item.since.elapsed());
                let name = m.display_name();
                let r = kit::row(
                    ui,
                    kit::RowSpec {
                        title: &name,
                        subtitle: &subtitle,
                        dot: theme::state_color(item.state),
                        icon: None,
                        trailing: Some(&waited),
                        active: false,
                        accent: Some(theme::state_color(item.state)),
                    },
                );
                if r.clicked() {
                    focus = Some(item.session);
                }
            }
        }

        ui.add_space(8.0);
        kit::section_header(ui, "Sessions", Some(c.sessions.len()), theme::MUTED);
        let card_h = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(egui::Id::new("usage-card-h")))
            .unwrap_or(170.0);
        let footer_h = card_h + 52.0;
        let list_h = (ui.available_height() - footer_h).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("sessions")
            .max_height(list_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (ti, tab) in self.tabs.iter().enumerate() {
                    for id in &tab.panes {
                        let Some(m) = c.sessions.get(id) else {
                            continue;
                        };
                        let mut name = m.display_name();
                        if tab.panes.len() > 1 {
                            name = format!("{name}  ·  split {}", ti + 1);
                        }
                        let place = m
                            .branch
                            .clone()
                            .unwrap_or_else(|| short_path(&m.cwd.to_string_lossy()));
                        let (subtitle, trailing) = if m.is_claude() {
                            (
                                format!("{} · {place}", kit::state_label(m.state())),
                                m.status.cost_usd.map(|c| format!("${c:.2}")),
                            )
                        } else {
                            let s = match (m.command_running, m.last_exit) {
                                (true, _) => "Running".to_string(),
                                (false, Some(c)) if c != 0 => format!("Exit {c}"),
                                _ => "Shell".to_string(),
                            };
                            (format!("{s} · {place}"), None)
                        };
                        let subtitle = if m.muted {
                            format!("{subtitle} · muted")
                        } else {
                            subtitle
                        };
                        let resp = kit::row(
                            ui,
                            kit::RowSpec {
                                title: &name,
                                subtitle: &subtitle,
                                dot: theme::state_color(m.state()),
                                icon: Some(if m.is_claude() {
                                    Icon::Sparkle
                                } else {
                                    Icon::Terminal
                                }),
                                trailing: trailing.as_deref(),
                                active: self.active == Some(*id) && !self.grid_mode,
                                accent: None,
                            },
                        );
                        if resp.clicked() {
                            focus = Some(*id);
                        }
                        let id = *id;
                        resp.context_menu(|ui| {
                            if ui
                                .button(if m.muted {
                                    "Unmute notifications"
                                } else {
                                    "Mute notifications"
                                })
                                .clicked()
                            {
                                MUTE_TOGGLE.with(|t| t.set(Some(id)));
                                ui.close();
                            }
                            if ui.button("Raise priority").clicked() {
                                PRIORITY.with(|t| t.set(Some((id, 1))));
                                ui.close();
                            }
                            if ui.button("Lower priority").clicked() {
                                PRIORITY.with(|t| t.set(Some((id, -1))));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Close session").clicked() {
                                close = Some(id);
                                ui.close();
                            }
                        });
                    }
                }
            });
        drop(c);

        // Footer: live usage card above the view toggles.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if kit::icon_button(ui, Icon::Chart, "Usage dashboard", self.usage_mode).clicked() {
                    actions.push(Action::Usage);
                }
                if kit::icon_button(ui, Icon::Sliders, "Settings", self.settings_open).clicked() {
                    actions.push(Action::Settings);
                }
                if kit::icon_button(ui, Icon::Grid, "Session grid", self.grid_mode).clicked() {
                    actions.push(Action::ToggleGrid);
                }
                if kit::icon_button(ui, Icon::Search, "Command palette", self.palette.is_some())
                    .clicked()
                {
                    actions.push(Action::Palette);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(sc) = self.sc(Action::Palette) {
                        kit::kbd(ui, &sc);
                    }
                });
            });
            ui.add_space(6.0);
            // A bottom-up layout would stretch the card; give it a box of its
            // own measured height (from the previous frame) laid out top-down.
            let hid = egui::Id::new("usage-card-h");
            let h = ui.ctx().data(|d| d.get_temp::<f32>(hid)).unwrap_or(170.0);
            let w = ui.available_width();
            let r = ui.allocate_ui_with_layout(
                egui::vec2(w, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.usage_card(ui),
            );
            let card = r.inner;
            ui.ctx()
                .data_mut(|d| d.insert_temp(hid, card.rect.height()));
            if card.clicked() {
                actions.push(Action::Usage);
            }
        });

        if let Some(id) = MUTE_TOGGLE.with(|t| t.take())
            && let Some(m) = self.core.lock().sessions.get_mut(&id)
        {
            m.muted = !m.muted;
        }
        if let Some((id, d)) = PRIORITY.with(|t| t.take())
            && let Some(m) = self.core.lock().sessions.get_mut(&id)
        {
            m.priority = (m.priority + d).clamp(-3, 3);
        }
        if let Some(id) = focus {
            self.focus(id);
        }
        if let Some(id) = close {
            self.close(id);
        }
        for a in actions {
            self.run(a);
        }
    }

    /// Sidebar card shown only while an update is available, installing,
    /// ready to restart, or failed.
    fn update_card(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        use theme::tokens as t;
        let state = self.update.lock().clone();
        let mut action = None;
        let (title, body, tone) = match &state {
            UpdateState::Available(r) => (
                format!("Update available · v{}", r.version()),
                String::new(),
                t::ACCENT_HOVER,
            ),
            UpdateState::Working(msg) => ("Updating Promptly".into(), msg.clone(), theme::BLUE),
            UpdateState::Ready { version, .. } => (
                format!("v{version} installed"),
                "Restart to finish. Sessions are offered back on launch.".into(),
                theme::GREEN,
            ),
            UpdateState::Failed(e) => ("Update failed".into(), e.clone(), theme::RED),
            UpdateState::Idle | UpdateState::Checking | UpdateState::UpToDate => return None,
        };
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(tone.gamma_multiply(0.10))
            .stroke(egui::Stroke::new(1.0, tone.gamma_multiply(0.35)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).size(13.0).color(t::TEXT));
                    if !body.is_empty() {
                        ui.label(RichText::new(body).size(11.5).color(t::TEXT_2));
                    }
                    ui.add_space(6.0);
                    match &state {
                        UpdateState::Available(r) => {
                            ui.horizontal(|ui| {
                                if kit::primary_button(
                                    ui,
                                    Some(Icon::Refresh),
                                    "Update",
                                    None,
                                    false,
                                )
                                .clicked()
                                {
                                    action = Some(Action::InstallUpdate);
                                }
                                if ui.button("What’s new").clicked() {
                                    open_external(&r.html_url);
                                }
                            });
                        }
                        UpdateState::Working(_) => {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                        UpdateState::Ready { target, exe, .. } => {
                            if kit::primary_button(
                                ui,
                                Some(Icon::Refresh),
                                "Restart now",
                                None,
                                false,
                            )
                            .clicked()
                            {
                                self.save_layout();
                                promptly_core::update::relaunch(target, exe);
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        UpdateState::Failed(_) => {
                            ui.horizontal(|ui| {
                                if ui.button("Try again").clicked() {
                                    action = Some(Action::CheckUpdates);
                                }
                                if ui.button("Dismiss").clicked() {
                                    *self.update.lock() = UpdateState::Idle;
                                }
                            });
                        }
                        _ => {}
                    }
                });
            });
        action
    }

    /// Sidebar card: plan limits with live countdowns, plus today's spend.
    fn usage_card(&self, ui: &mut egui::Ui) -> egui::Response {
        use theme::tokens as t;
        let now = promptly_core::usage::now_secs();
        let (five, week, spend, burn, any_working) = {
            let c = self.core.lock();
            let (five, week) = c.account.current(now);
            let spend: f64 = c.sessions.values().filter_map(|m| m.status.cost_usd).sum();
            let burn: f64 = c
                .sessions
                .values()
                .filter_map(|m| m.cost_series.rate_per_hour(now as f64, 1800.0))
                .sum();
            let working = c
                .sessions
                .values()
                .any(|m| m.state() == SessionState::Working);
            (five, week, spend, burn, working)
        };
        let reduce = self.reduce_motion;
        let resp = egui::Frame::new()
            .fill(t::BG_ELEVATED)
            .stroke(egui::Stroke::new(1.0, t::BORDER))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                // The footer lays out bottom-up; the card itself reads top-down.
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Usage").size(12.5).color(t::TEXT));
                        if any_working {
                            // Live indicator: a softly pulsing dot while tokens flow.
                            let phase = if reduce {
                                1.0
                            } else {
                                (ui.input(|i| i.time) * 2.4).sin() as f32 * 0.35 + 0.65
                            };
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                r.center(),
                                3.5,
                                theme::GREEN.gamma_multiply(phase),
                            );
                            ui.label(RichText::new("live").size(11.0).color(t::TEXT_3));
                            ui.ctx().request_repaint_after(Duration::from_millis(50));
                        }
                    });
                    ui.add_space(4.0);
                    charts::limit_row(ui, "5-hour", five, now, reduce);
                    ui.add_space(6.0);
                    charts::limit_row(ui, "Weekly", week, now, reduce);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let s = charts::tween(ui, "spend-total", spend as f32, reduce);
                        ui.label(RichText::new(format!("${s:.2}")).size(15.0).color(t::TEXT));
                        ui.label(RichText::new("open sessions").size(11.0).color(t::TEXT_3));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = charts::tween(ui, "burn-total", burn.max(0.0) as f32, reduce);
                            ui.label(
                                RichText::new(format!("${b:.2}/h"))
                                    .size(11.5)
                                    .color(t::TEXT_2),
                            )
                            .on_hover_text("Spend rate over the last 30 minutes");
                        });
                    });
                });
            })
            .response;
        resp.interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Open the usage dashboard")
    }

    /// Full usage dashboard: limits with history, today's tokens, live sessions.
    fn usage_view(&mut self, ui: &mut egui::Ui) {
        use promptly_core::usage::{fmt_tokens, fmt_until};
        use theme::tokens as t;
        let rect = ui.available_rect_before_wrap();
        window_drag_zone(
            ui,
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 40.0)),
            "usage-drag",
        );
        let now = promptly_core::usage::now_secs();
        let nowf = now as f64;
        let reduce = self.reduce_motion;
        let mut focus = None;
        let c = self.core.lock();
        let (five, week) = c.account.current(now);
        let five_hist = c.account.five_hour_history.window(nowf, 5.0 * 3600.0);
        let week_hist = c.account.seven_day_history.window(nowf, 7.0 * 86_400.0);
        let observed = c.account.observed_at;
        let daily = c.daily.clone();
        struct Row {
            id: PaneId,
            name: String,
            state: SessionState,
            cost: Option<f64>,
            tokens: Option<u64>,
            ctx: Option<f32>,
            lines: (u64, u64),
            burn: Option<f64>,
            series: Vec<(f64, f64)>,
        }
        let rows: Vec<Row> = c
            .sessions
            .iter()
            .filter(|(_, m)| m.is_claude())
            .map(|(id, m)| Row {
                id: *id,
                name: m.display_name(),
                state: m.state(),
                cost: m.status.cost_usd,
                tokens: m
                    .status
                    .input_tokens
                    .zip(m.status.output_tokens)
                    .map(|(a, b)| a + b),
                ctx: m.context_percent(),
                lines: (
                    m.status.lines_added.unwrap_or(0),
                    m.status.lines_removed.unwrap_or(0),
                ),
                burn: m.cost_series.rate_per_hour(nowf, 1800.0),
                series: m.cost_series.window(nowf, 3600.0),
            })
            .collect();
        drop(c);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 24, right: 24, top: 14, bottom: 24 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Usage").size(20.0).color(t::TEXT));
                    let note = match observed {
                        Some(o) => format!("Live from Claude Code · updated {} ago", fmt_until(now - o)),
                        None => "Start a Claude session to receive plan limits".into(),
                    };
                    ui.label(RichText::new(note).size(12.0).color(t::TEXT_3));
                });
                ui.add_space(14.0);

                // Plan limit cards
                let full = ui.available_width();
                let half = (full - 16.0) / 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (label, w, hist, span) in [
                        ("5-hour limit", five, &five_hist, 5.0 * 3600.0),
                        ("Weekly limit", week, &week_hist, 7.0 * 86_400.0),
                    ] {
                        egui::Frame::new()
                            .fill(t::BG_SIDEBAR)
                            .stroke(egui::Stroke::new(1.0, t::BORDER))
                            .corner_radius(egui::CornerRadius::same(12))
                            .inner_margin(egui::Margin::same(16))
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                ui.set_width(half - 34.0);
                                ui.label(RichText::new(label).size(12.5).color(t::TEXT_2));
                                match w {
                                    Some(w) => {
                                        let pct = charts::tween(ui, ("big", label), w.used_percentage, reduce);
                                        let (level, color) = charts::limit_level(w.used_percentage);
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(format!("{pct:.0}%")).size(34.0).color(t::TEXT));
                                            ui.vertical(|ui| {
                                                ui.add_space(8.0);
                                                ui.label(RichText::new(level).size(12.0).color(color));
                                                ui.label(RichText::new("used").size(11.5).color(t::TEXT_3));
                                            });
                                        });
                                        let fill = if level == "OK" { t::TEXT_2 } else { color };
                                        charts::limit_bar(ui, pct / 100.0, fill, ui.available_width());
                                        ui.add_space(6.0);
                                        let at = chrono::DateTime::from_timestamp(w.resets_at, 0)
                                            .map(|d| d.with_timezone(&chrono::Local).format("%a %H:%M").to_string())
                                            .unwrap_or_default();
                                        ui.label(
                                            RichText::new(format!("Resets in {} · {at}", fmt_until(w.resets_at - now)))
                                                .size(12.0)
                                                .color(t::TEXT_2),
                                        );
                                        ui.add_space(8.0);
                                        let size = egui::vec2(ui.available_width(), 56.0);
                                        charts::sparkline(ui, hist, (nowf - span, nowf), Some(100.0), size, theme::BLUE, |x, y| {
                                            let at = chrono::DateTime::from_timestamp(x as i64, 0)
                                                .map(|d| d.with_timezone(&chrono::Local).format("%a %H:%M").to_string())
                                                .unwrap_or_default();
                                            format!("{at}\n{y:.0}% used")
                                        });
                                    }
                                    None => {
                                        ui.label(RichText::new("—").size(34.0).color(t::TEXT_3));
                                        ui.label(RichText::new("Reported by Claude Code once a session runs.").size(12.0).color(t::TEXT_3));
                                    }
                                }
                                });
                            });
                    }
                });
                ui.add_space(16.0);

                // KPI tiles
                let spend: f64 = rows.iter().filter_map(|r| r.cost).sum();
                let burn: f64 = rows.iter().filter_map(|r| r.burn).sum();
                let lines = rows.iter().fold((0, 0), |a, r| (a.0 + r.lines.0, a.1 + r.lines.1));
                let tile_w = (full - 3.0 * 12.0) / 4.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let s = charts::tween(ui, "dash-spend", spend as f32, reduce);
                    charts::stat_tile(ui, "Spend, open sessions", &format!("${s:.2}"), &format!("burning ${:.2}/h", burn.max(0.0)), tile_w);
                    let (tok, detail, req) = match &daily {
                        Some(d) => {
                            let tt = d.total();
                            (
                                fmt_tokens(charts::tween(ui, "dash-tok", tt.total() as f32, reduce) as u64),
                                format!("{} out · {} cached", fmt_tokens(tt.output), fmt_tokens(tt.cache_read)),
                                (d.requests, d.sessions),
                            )
                        }
                        None => ("…".into(), "scanning transcripts".into(), (0, 0)),
                    };
                    charts::stat_tile(ui, "Tokens today, all sessions", &tok, &detail, tile_w);
                    charts::stat_tile(
                        ui,
                        "Requests today",
                        &req.0.to_string(),
                        &format!("{} sessions on this Mac", req.1),
                        tile_w,
                    );
                    charts::stat_tile(ui, "Lines changed", &format!("+{} −{}", lines.0, lines.1), "open sessions", tile_w);
                });
                ui.add_space(16.0);

                // Hourly tokens
                if let Some(d) = &daily {
                    egui::Frame::new()
                        .fill(t::BG_SIDEBAR)
                        .stroke(egui::Stroke::new(1.0, t::BORDER))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| { ui.vertical(|ui| {
                            ui.set_width(full - 32.0);
                            ui.label(RichText::new("Tokens by hour, today").size(12.5).color(t::TEXT_2));
                            ui.add_space(8.0);
                            let hour = ((now - d.day_start) / 3600).clamp(0, 23) as usize;
                            charts::hourly_bars(ui, &d.by_hour, hour, egui::vec2(full - 32.0, 120.0));
                            if !d.by_model.is_empty() {
                                ui.add_space(10.0);
                                for (model, tk) in &d.by_model {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(model).monospace().size(11.5).color(t::TEXT_1));
                                        ui.label(
                                            RichText::new(format!(
                                                "{} in · {} out · {} cache read · {} cache write",
                                                fmt_tokens(tk.input),
                                                fmt_tokens(tk.output),
                                                fmt_tokens(tk.cache_read),
                                                fmt_tokens(tk.cache_write)
                                            ))
                                            .size(11.5)
                                            .color(t::TEXT_3),
                                        );
                                    });
                                }
                            }
                        }); });
                    ui.add_space(16.0);
                }

                // Live sessions table
                egui::Frame::new()
                    .fill(t::BG_SIDEBAR)
                    .stroke(egui::Stroke::new(1.0, t::BORDER))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| { ui.vertical(|ui| {
                        ui.set_width(full - 32.0);
                        ui.label(RichText::new("Claude sessions").size(12.5).color(t::TEXT_2));
                        ui.add_space(6.0);
                        if rows.is_empty() {
                            ui.label(RichText::new("No Claude sessions open").color(t::TEXT_3));
                        }
                        let cols = [0.30, 0.12, 0.11, 0.11, 0.09, 0.09, 0.18];
                        let w = full - 32.0;
                        let header = ["Session", "Cost", "Tokens", "Rate", "Context", "Lines", "Last hour"];
                        ui.horizontal(|ui| {
                            for (i, h) in header.iter().enumerate() {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(w * cols[i] - 8.0, 18.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width(w * cols[i] - 8.0);
                                        ui.label(RichText::new(*h).size(11.0).color(t::TEXT_3))
                                    },
                                );
                            }
                        });
                        for r in &rows {
                            let resp = ui
                                .horizontal(|ui| {
                                    let cell = |ui: &mut egui::Ui, i: usize, text: RichText| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(w * cols[i] - 8.0, 30.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.set_min_width(w * cols[i] - 8.0);
                                                ui.add(egui::Label::new(text).truncate())
                                            },
                                        );
                                    };
                                    cell(ui, 0, RichText::new(format!("   {}", r.name)).color(t::TEXT_1));
                                    let cost = r.cost.map(|v| charts::tween(ui, ("row-cost", r.id), v as f32, reduce));
                                    cell(ui, 1, RichText::new(cost.map(|v| format!("${v:.2}")).unwrap_or("—".into())).color(t::TEXT));
                                    cell(ui, 2, RichText::new(r.tokens.map(fmt_tokens).unwrap_or("—".into())).color(t::TEXT_2));
                                    cell(ui, 3, RichText::new(r.burn.map(|b| format!("${b:.2}/h")).unwrap_or("—".into())).color(t::TEXT_2));
                                    cell(ui, 4, RichText::new(r.ctx.map(|p| format!("{p:.0}%")).unwrap_or("—".into())).color(t::TEXT_2));
                                    cell(ui, 5, RichText::new(format!("+{} −{}", r.lines.0, r.lines.1)).color(t::TEXT_2));
                                    let size = egui::vec2(w * cols[6] - 8.0, 22.0);
                                    charts::sparkline(ui, &r.series, (nowf - 3600.0, nowf), None, size, theme::state_color(r.state), |_, y| format!("${y:.2}"));
                                })
                                .response
                                .interact(egui::Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            // State dot, painted over the "●" glyph position.
                            ui.painter().circle_filled(
                                resp.rect.left_center() + egui::vec2(4.0, 0.0),
                                4.0,
                                theme::state_color(r.state),
                            );
                            if resp.clicked() {
                                focus = Some(r.id);
                            }
                        }
                    }); });
            });
        });
        if let Some(id) = focus {
            self.focus(id);
        }
    }

    fn pane_header(&mut self, ui: &mut egui::Ui, id: PaneId, is_active: bool) {
        let mut allow_clip = None;
        let mut actions = vec![];
        {
            let c = self.core.lock();
            let Some(m) = c.sessions.get(&id) else { return };
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
            window_drag_zone(ui, rect, ("pane-drag", id));
            ui.painter().line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                egui::Stroke::new(1.0, theme::tokens::BORDER),
            );
            let mut hui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let ui = &mut hui;
            ui.spacing_mut().item_spacing.x = 10.0;
            let st = m.state();
            if m.is_claude() {
                kit::state_pill(ui, kit::state_label(st), theme::state_color(st));
            } else {
                let (label, col) = match (m.command_running, m.last_exit) {
                    (true, _) => ("Running".to_string(), theme::BLUE),
                    (false, Some(c)) if c != 0 => (format!("Exit {c}"), theme::RED),
                    _ => ("Shell".to_string(), theme::MUTED),
                };
                kit::state_pill(ui, &label, col);
            }
            let title_col = if is_active {
                theme::tokens::TEXT
            } else {
                theme::tokens::TEXT_2
            };
            ui.label(RichText::new(m.display_name()).size(13.5).color(title_col))
                .on_hover_text(m.cwd.to_string_lossy());
            if let Some(b) = &m.branch {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                kit::paint_icon(ui, r, Icon::Branch, theme::tokens::TEXT_3);
                ui.add_space(-6.0);
                ui.label(RichText::new(b).size(12.5).color(theme::tokens::TEXT_2));
            }
            if m.is_claude() && !m.hooks_injected {
                kit::pill(ui, "transcript-only", theme::AMBER)
                    .on_hover_text("Hooks are not injected; state is derived from the transcript.");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if kit::icon_button(ui, Icon::PanelRight, "Review changes", self.review_open)
                    .clicked()
                {
                    actions.push(Action::ToggleReview);
                }
                if kit::icon_button(ui, Icon::Compose, "Composer", self.composer_open).clicked() {
                    actions.push(Action::ToggleComposer);
                }
                if kit::icon_button(ui, Icon::Search, "Find in scrollback", false).clicked() {
                    actions.push(Action::Search);
                }
                if !m.is_claude() {
                    return;
                }
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 12.0;
                let muted = theme::tokens::TEXT_3;
                if let Some(model) = m.status.model.clone().or_else(|| m.stats.model.clone()) {
                    ui.label(RichText::new(model).size(12.0).color(muted));
                }
                match m.status.cost_usd {
                    Some(c) => ui.label(
                        RichText::new(format!("${c:.2}"))
                            .size(12.5)
                            .color(theme::tokens::TEXT_2),
                    ),
                    None => ui.label(RichText::new("$ —").size(12.5).color(muted)),
                }
                .on_hover_text("Session cost reported by Claude Code");
                let pct = m.context_percent();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let label = pct
                        .map(|p| format!("{p:.0}%"))
                        .unwrap_or_else(|| "—".into());
                    ui.label(RichText::new(label).size(12.5).color(theme::tokens::TEXT_2));
                    let col = match pct {
                        Some(p) if p > 85.0 => theme::RED,
                        Some(p) if p > 65.0 => theme::AMBER,
                        _ => theme::tokens::TEXT_2,
                    };
                    kit::meter(ui, pct.unwrap_or(0.0) / 100.0, col, 44.0);
                })
                .response
                .on_hover_text("Context window used");
                let elapsed = m
                    .tracker
                    .turn_elapsed()
                    .unwrap_or_else(|| m.started.elapsed());
                ui.label(RichText::new(fmt_dur(elapsed)).size(12.5).color(muted))
                    .on_hover_text("Current or last turn");
            });
        }
        // Callouts under the header.
        let c = self.core.lock();
        if let Some(m) = c.sessions.get(&id) {
            if m.pending_clipboard.is_some() {
                callout(ui, theme::AMBER, |ui| {
                    ui.label("This program wants to write to your clipboard.");
                    if ui.button("Allow for this session").clicked() {
                        allow_clip = Some(true);
                    }
                    if ui.button("Deny").clicked() {
                        allow_clip = Some(false);
                    }
                });
            }
            if let (SessionState::WaitingPermission, Some(d)) = (m.state(), &m.attention_detail) {
                let d = d.clone();
                callout(ui, theme::AMBER, |ui| {
                    ui.label(
                        RichText::new("Approve in the terminal below:")
                            .color(theme::tokens::TEXT_1),
                    );
                    ui.label(RichText::new(d).monospace().color(theme::tokens::TEXT));
                });
            }
        }
        drop(c);
        if let Some(allow) = allow_clip {
            let mut c = self.core.lock();
            if let Some(m) = c.sessions.get_mut(&id) {
                m.clipboard_allowed = Some(allow);
                if let (true, Some(t)) = (allow, m.pending_clipboard.take()) {
                    self.ctx.copy_text(t);
                }
                m.pending_clipboard = None;
            }
        }
        if !actions.is_empty() {
            self.active = Some(id);
        }
        for a in actions {
            self.run(a);
        }
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, id: PaneId, overlay_open: bool) {
        let is_active = self.active == Some(id);
        self.pane_header(ui, id, is_active);
        let is_claude = self
            .core
            .lock()
            .sessions
            .get(&id)
            .is_some_and(|m| m.is_claude());
        let request_focus = self.active == Some(id) && self.focus_terminal;
        // Search bar
        let mut search_action = None;
        if let Some(e) = self.panes.get_mut(&id)
            && let Some(s) = e.view.search.as_mut()
        {
            egui::Frame::new()
                .fill(theme::tokens::BG_ELEVATED)
                .stroke(egui::Stroke::new(1.0, theme::tokens::BORDER))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 4))
                .outer_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (ir, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        kit::paint_icon(ui, ir, Icon::Search, theme::tokens::TEXT_3);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut s.query)
                                .frame(egui::Frame::NONE)
                                .desired_width(260.0)
                                .hint_text("Find in scrollback (regex)"),
                        );
                        if s.focus_requested {
                            r.request_focus();
                            s.focus_requested = false;
                        }
                        if r.changed() {
                            search_action = Some(0);
                        }
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if enter {
                            search_action = Some(-1);
                            r.request_focus();
                        }
                        ui.label(
                            RichText::new(&s.status)
                                .size(11.5)
                                .color(theme::tokens::TEXT_3),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::icon_button(ui, Icon::Close, "Close (Esc)", false).clicked()
                                || (r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)))
                            {
                                search_action = Some(9);
                            }
                            if ui.small_button("Next").clicked() {
                                search_action = Some(1);
                            }
                            if ui.small_button("Previous").on_hover_text("Enter").clicked() {
                                search_action = Some(-1);
                            }
                        });
                    });
                });
        }
        if let (Some(a), Some(e)) = (search_action, self.panes.get_mut(&id)) {
            match a {
                0 => term_view::invalidate_search(&mut e.view),
                9 => {
                    e.view.search = None;
                    self.focus_terminal = true;
                }
                d => term_view::search_step(&e.pane, &mut e.view, d > 0),
            }
        }
        let searching = self.panes.get(&id).is_some_and(|e| e.view.search.is_some());
        let opts = ViewOptions {
            font: self.font(),
            claude_pane: is_claude,
            option_as_meta: self.option_as_meta,
            request_focus: request_focus && !searching,
            keyboard_blocked: overlay_open,
        };
        let Some(e) = self.panes.get_mut(&id) else {
            return;
        };
        let out = term_view::show(ui, &e.pane, &mut e.view, &opts);
        if request_focus {
            self.focus_terminal = false;
        }
        if out.has_focus && self.active != Some(id) {
            self.active = Some(id);
        }
        if let Some(t) = out.copy {
            self.ctx.copy_text(t);
        }
        if let Some(url) = out.open_url {
            open_external(&url);
        }
        if let Some(p) = out.paste_for_review {
            self.paste_review = Some((id, p));
        }
        if out.typed {
            let mut c = self.core.lock();
            let tr = c
                .sessions
                .get_mut(&id)
                .and_then(|m| m.tracker.acknowledge());
            c.apply(id, tr);
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let mut actions = vec![];
        let rect = ui.available_rect_before_wrap();
        window_drag_zone(
            ui,
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 40.0)),
            "empty-drag",
        );
        let w = 360.0;
        let col = egui::Rect::from_center_size(rect.center(), egui::vec2(w, 300.0));
        let mut cui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(col)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let (r, _) = cui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
        kit::paint_icon(&cui, r, Icon::Sparkle, theme::tokens::ACCENT_HOVER);
        cui.add_space(8.0);
        cui.label(
            RichText::new("Start a session")
                .size(20.0)
                .color(theme::tokens::TEXT),
        );
        cui.label(
            RichText::new(
                "Run Claude Code with attention tracking, worktrees and review built in.",
            )
            .size(13.0)
            .color(theme::tokens::TEXT_2),
        );
        cui.add_space(16.0);
        let sc = self.sc(Action::NewClaude);
        if kit::primary_button(
            &mut cui,
            Some(Icon::Sparkle),
            "New Claude session",
            sc.as_deref(),
            true,
        )
        .clicked()
        {
            actions.push(Action::NewClaude);
        }
        cui.add_space(4.0);
        for (icon, label, a) in [
            (Icon::Terminal, "New shell", Action::NewShell),
            (Icon::Clock, "Resume a session", Action::History),
            (Icon::Fork, "Fan out tasks", Action::FanOut),
        ] {
            let sc = self.sc(a);
            if kit::ghost_button(&mut cui, icon, label, sc.as_deref()).clicked() {
                actions.push(a);
            }
        }
        for a in actions {
            self.run(a);
        }
    }

    fn panes_area(&mut self, ui: &mut egui::Ui, overlay_open: bool) {
        if self.tabs.is_empty() {
            self.empty_state(ui);
            return;
        }
        let tab = &self.tabs[self.active_tab];
        let ids = tab.panes.clone();
        let vertical = tab.vertical;
        let rect = ui.available_rect_before_wrap();
        let n = ids.len() as f32;
        let gap = 1.0;
        for (i, id) in ids.iter().enumerate() {
            let r = if vertical {
                let h = rect.height() / n;
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(0.0, h * i as f32),
                    egui::vec2(rect.width(), h - gap),
                )
            } else {
                let w = rect.width() / n;
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(w * i as f32, 0.0),
                    egui::vec2(w - gap, rect.height()),
                )
            };
            if i > 0 {
                let (a, b) = if vertical {
                    (
                        r.left_top() - egui::vec2(0.0, gap),
                        r.right_top() - egui::vec2(0.0, gap),
                    )
                } else {
                    (
                        r.left_top() - egui::vec2(gap, 0.0),
                        r.left_bottom() - egui::vec2(gap, 0.0),
                    )
                };
                ui.painter()
                    .line_segment([a, b], egui::Stroke::new(1.0, theme::tokens::BORDER));
            }
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("pane", *id)));
            self.pane_ui(&mut child, *id, overlay_open);
        }
        ui.advance_cursor_after_rect(rect);
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        window_drag_zone(
            ui,
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 40.0)),
            "grid-drag",
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(
                RichText::new("All sessions")
                    .size(15.0)
                    .color(theme::tokens::TEXT),
            );
        });
        ui.add_space(8.0);
        let ids = self.ordered_panes();
        let cols = ((ids.len() as f32).sqrt().ceil() as usize).clamp(1, 4);
        let gap = 12.0;
        let w = ((ui.available_width() - 32.0 - gap * (cols as f32 - 1.0)) / cols as f32)
            .clamp(220.0, 560.0);
        let mut focus = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for chunk in ids.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.spacing_mut().item_spacing.x = gap;
                    for id in chunk {
                        let (name, st, detail, branch, is_claude) = {
                            let c = self.core.lock();
                            let Some(m) = c.sessions.get(id) else {
                                continue;
                            };
                            (
                                m.display_name(),
                                m.state(),
                                m.attention_detail.clone(),
                                m.branch.clone(),
                                m.is_claude(),
                            )
                        };
                        let lines = self
                            .panes
                            .get(id)
                            .map(|e| term_view::screen_text(&e.pane, 12))
                            .unwrap_or_default();
                        let (r, resp) =
                            ui.allocate_exact_size(egui::vec2(w, 250.0), egui::Sense::click());
                        let hovered = resp.hovered();
                        let p = ui.painter();
                        p.rect(
                            r,
                            egui::CornerRadius::same(10),
                            theme::tokens::BG_SIDEBAR,
                            egui::Stroke::new(
                                1.0,
                                if hovered {
                                    theme::state_color(st)
                                } else {
                                    theme::tokens::BORDER
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        let mut cu = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(r.shrink(12.0))
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        cu.horizontal(|ui| {
                            let label = if is_claude {
                                kit::state_label(st)
                            } else {
                                "Shell"
                            };
                            kit::state_pill(ui, label, theme::state_color(st));
                            let g = kit::elide(
                                ui,
                                &name,
                                egui::FontId::proportional(13.0),
                                theme::tokens::TEXT,
                                ui.available_width() - 8.0,
                            );
                            ui.label(g);
                        });
                        let sub = match (&detail, &branch) {
                            (Some(d), _) if st.needs_attention() => d.clone(),
                            (_, Some(b)) => b.clone(),
                            _ => String::new(),
                        };
                        cu.label(
                            RichText::new(sub)
                                .size(11.5)
                                .color(if st.needs_attention() {
                                    theme::AMBER
                                } else {
                                    theme::tokens::TEXT_3
                                }),
                        );
                        let preview = cu.available_rect_before_wrap();
                        cu.painter().rect_filled(
                            preview,
                            egui::CornerRadius::same(6),
                            theme::tokens::BG_MAIN,
                        );
                        let mut y = preview.min.y + 6.0;
                        for l in lines {
                            if y > preview.max.y - 12.0 {
                                break;
                            }
                            let g = kit::elide(
                                &cu,
                                &l,
                                egui::FontId::monospace(10.5),
                                theme::tokens::TEXT_2,
                                preview.width() - 16.0,
                            );
                            cu.painter().galley(
                                egui::pos2(preview.min.x + 8.0, y),
                                g,
                                theme::tokens::TEXT_2,
                            );
                            y += 14.0;
                        }
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            focus = Some(*id);
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
        if let Some(id) = focus {
            self.focus(id);
        }
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        // Command palette
        if let Some(p) = self.palette.as_mut() {
            let mut run = None;
            let mut close = false;
            let shortcuts = self.shortcuts.clone();
            let modal = egui::Modal::new(egui::Id::new("palette"))
                .frame(kit::floating_frame())
                .show(ctx, |ui| {
                    ui.set_width(560.0);
                    let q = p.query.to_ascii_lowercase();
                    let matches: Vec<_> = ACTIONS
                        .iter()
                        .filter(|(a, _, label, _)| {
                            *a != Action::Palette
                                && crate::composer::fuzzy(&label.to_ascii_lowercase(), &q)
                        })
                        .collect();
                    ui.input_mut(|i| {
                        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                            p.sel = (p.sel + 1).min(matches.len().saturating_sub(1));
                        }
                        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                            p.sel = p.sel.saturating_sub(1);
                        }
                        if i.consume_key(Modifiers::NONE, Key::Escape) {
                            close = true;
                        }
                        if i.consume_key(Modifiers::NONE, Key::Enter) {
                            run = matches.get(p.sel).map(|m| m.0);
                        }
                    });
                    search_field(ui, &mut p.query, "Type a command…");
                    ui.add_space(6.0);
                    p.sel = p.sel.min(matches.len().saturating_sub(1));
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            if matches.is_empty() {
                                ui.label(
                                    RichText::new("No matching commands")
                                        .color(theme::tokens::TEXT_3),
                                );
                            }
                            for (i, (a, _, label, _)) in matches.iter().enumerate() {
                                let sc = shortcuts
                                    .iter()
                                    .find(|(x, _)| x == a)
                                    .map(|(_, s)| ctx.format_shortcut(s));
                                if list_item(ui, label, None, sc.as_deref(), i == p.sel).clicked() {
                                    run = Some(*a);
                                }
                            }
                        });
                    ui.add_space(4.0);
                    footer_hints(ui, &[("↑↓", "navigate"), ("↵", "run"), ("esc", "close")]);
                });
            if modal.should_close() {
                close = true;
            }
            if close || run.is_some() {
                self.palette = None;
                self.focus_terminal = true;
            }
            if let Some(a) = run {
                self.run(a);
            }
        }

        // History / resume picker
        if let Some(h) = self.history.as_mut() {
            let mut pick = None;
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::new("history"))
                .frame(kit::floating_frame())
                .show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.input_mut(|i| {
                        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                            h.sel = (h.sel + 1).min(h.results.len().saturating_sub(1));
                        }
                        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                            h.sel = h.sel.saturating_sub(1);
                        }
                        if i.consume_key(Modifiers::NONE, Key::Escape) {
                            close = true;
                        }
                        if i.consume_key(Modifiers::NONE, Key::Enter) {
                            pick = h.results.get(h.sel).cloned();
                        }
                    });
                    if search_field(
                        ui,
                        &mut h.query,
                        "Search past Claude sessions by text or folder",
                    )
                    .changed()
                    {
                        h.dirty = true;
                        h.sel = 0;
                    }
                    if h.dirty
                        && let Some(idx) = self.index.lock().as_ref()
                    {
                        h.results = idx.search(&h.query, 50).unwrap_or_default();
                        h.dirty = false;
                    }
                    ui.add_space(6.0);
                    if self.index.lock().is_none() {
                        ui.label(
                            RichText::new("Indexing transcripts…").color(theme::tokens::TEXT_3),
                        );
                    } else if h.results.is_empty() {
                        ui.label(RichText::new("No sessions found").color(theme::tokens::TEXT_3));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            for (i, rec) in h.results.iter().enumerate() {
                                let date = rec
                                    .last_timestamp
                                    .get(..16)
                                    .unwrap_or(&rec.last_timestamp)
                                    .replace('T', " ");
                                let title = if rec.first_prompt.is_empty() {
                                    "(no prompt)"
                                } else {
                                    rec.first_prompt.as_str()
                                };
                                let sub = format!(
                                    "{}  ·  {date}  ·  {} turns",
                                    short_path(&rec.cwd),
                                    rec.turns
                                );
                                if list_item(ui, title, Some(&sub), None, i == h.sel).clicked() {
                                    pick = Some(rec.clone());
                                }
                            }
                        });
                    ui.add_space(4.0);
                    footer_hints(ui, &[("↑↓", "navigate"), ("↵", "resume"), ("esc", "close")]);
                });
            if modal.should_close() {
                close = true;
            }
            if let Some(rec) = pick {
                self.history = None;
                let cwd = PathBuf::from(&rec.cwd);
                if cwd.is_dir() {
                    self.spawn(NewSession {
                        cwd: Some(cwd),
                        claude: true,
                        resume: Some(rec.session_id),
                        ..Default::default()
                    });
                } else {
                    self.toast(format!("{} no longer exists", rec.cwd));
                }
            } else if close {
                self.history = None;
                self.focus_terminal = true;
            }
        }

        // Fan-out
        if let Some(f) = self.fanout.as_mut() {
            let mut launch = false;
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::new("fanout"))
                .frame(kit::floating_frame())
                .show(ctx, |ui| {
                    ui.set_width(560.0);
                    ui.label(
                        RichText::new("Fan out tasks")
                            .size(16.0)
                            .color(theme::tokens::TEXT),
                    );
                    ui.label(
                        RichText::new("One task per line. Each runs in its own Claude session.")
                            .color(theme::tokens::TEXT_2),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut f.tasks)
                            .desired_rows(7)
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::same(10))
                            .hint_text("Add rate limiting to the API\nWrite tests for the parser"),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Repository")
                            .size(12.0)
                            .color(theme::tokens::TEXT_2),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut f.cwd)
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(10, 6)),
                    );
                    ui.add_space(4.0);
                    ui.checkbox(&mut f.worktrees, "Give each session its own git worktree");
                    ui.add_space(10.0);
                    let n = f.tasks.lines().filter(|l| !l.trim().is_empty()).count();
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape))
                        {
                            close = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = match n {
                                0 => "Add a task to launch".to_string(),
                                1 => "Launch 1 session".to_string(),
                                n => format!("Launch {n} sessions"),
                            };
                            ui.add_enabled_ui(n > 0, |ui| {
                                if kit::primary_button(ui, Some(Icon::Fork), &label, None, false)
                                    .clicked()
                                {
                                    launch = true;
                                }
                            });
                        });
                    });
                });
            if modal.should_close() {
                close = true;
            }
            if launch {
                let f = self.fanout.take().unwrap();
                let cwd = config::expand_tilde(Path::new(f.cwd.trim()));
                for task in f.tasks.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    self.spawn(NewSession {
                        cwd: Some(cwd.clone()),
                        claude: true,
                        worktree: f.worktrees.then(|| task.to_string()),
                        prompt: Some(task.to_string()),
                        ..Default::default()
                    });
                }
                self.grid_mode = true;
            } else if close {
                self.fanout = None;
            }
        }

        // Paste preview
        if let Some((id, text)) = self.paste_review.clone() {
            let mut decision = None;
            egui::Modal::new(egui::Id::new("paste"))
                .frame(kit::floating_frame())
                .show(ctx, |ui| {
                    ui.set_width(580.0);
                    let risk = sanitize::PasteRisk::of(&text);
                    ui.label(
                        RichText::new("Review this paste")
                            .size(16.0)
                            .color(theme::tokens::TEXT),
                    );
                    let mut why = vec![format!("{} lines", risk.lines)];
                    if risk.control_chars > 0 {
                        why.push(format!("{} hidden control characters", risk.control_chars));
                    }
                    if risk.paste_terminator {
                        why.push("a sequence that could escape paste mode".into());
                    }
                    ui.label(
                        RichText::new(format!("Contains {}.", why.join(", ")))
                            .color(theme::tokens::TEXT_2),
                    );
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(theme::tokens::BG_MAIN)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(280.0)
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(sanitize::visible_controls(&text))
                                            .monospace(),
                                    );
                                });
                        });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape))
                        {
                            decision = Some(None);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::primary_button(ui, None, "Paste safely", None, false)
                                .on_hover_text("Removes control characters")
                                .clicked()
                            {
                                decision = Some(Some(sanitize::strip_paste_controls(&text)));
                            }
                            if ui.button("Paste as is").clicked() {
                                decision = Some(Some(text.clone()));
                            }
                        });
                    });
                });
            if let Some(d) = decision {
                self.paste_review = None;
                self.focus_terminal = true;
                if let (Some(t), Some(e)) = (d, self.panes.get(&id)) {
                    let bracketed = e
                        .pane
                        .term
                        .lock()
                        .mode()
                        .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
                    e.pane.write(sanitize::encode_paste(&t, bracketed));
                }
            }
        }

        // Restore banner
        if let Some(l) = &self.restore {
            let n: usize = l.tabs.iter().map(Vec::len).sum();
            let mut answer = None;
            egui::Area::new(egui::Id::new("restore"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 52.0])
                .show(ctx, |ui| {
                    kit::floating_frame()
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(16.0, 16.0),
                                    egui::Sense::hover(),
                                );
                                kit::paint_icon(ui, r, Icon::Refresh, theme::tokens::TEXT_2);
                                ui.label(
                                    RichText::new(format!(
                                        "Restore {n} session{} from last time?",
                                        if n == 1 { "" } else { "s" }
                                    ))
                                    .color(theme::tokens::TEXT),
                                );
                                ui.add_space(8.0);
                                if ui.button("Dismiss").clicked() {
                                    answer = Some(false);
                                }
                                if kit::primary_button(ui, None, "Restore", None, false).clicked() {
                                    answer = Some(true);
                                }
                            });
                        });
                });
            if let Some(a) = answer {
                let l = self.restore.take().unwrap();
                if a {
                    self.restore_layout(l);
                }
            }
        }

        self.settings_window(ctx);
        self.log_window(ctx);
        self.transcript_window(ctx);

        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(6) {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
                    .show(ctx, |ui| {
                        kit::floating_frame()
                            .corner_radius(egui::CornerRadius::same(18))
                            .inner_margin(egui::Margin::symmetric(16, 9))
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg.as_str()).color(theme::tokens::TEXT));
                            });
                    });
                ctx.request_repaint_after(Duration::from_secs(1));
            } else {
                self.toast = None;
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut actions = vec![];
        egui::Window::new("Settings").open(&mut open).default_width(560.0).show(ctx, |ui| {
            ui.heading("Claude");
            if self.hooks_locked {
                ui.colored_label(theme::AMBER, "Hooks are locked down by organization-managed settings. Promptly uses transcript tailing instead.");
            }
            ui.checkbox(&mut self.cfg.claude.inject_hooks, "Inject session-scoped hooks (applies to new sessions)");
            ui.checkbox(&mut self.cfg.claude.wrap_statusline, "Wrap the status line to read cost and context % (chains to your own)");
            ui.collapsing("Injected hooks", |ui| {
                ui.label("Passed with --settings for each session. Your ~/.claude/settings.json is never modified; your own hooks keep running.");
                let json = match &self.ctl_path {
                    Some(ctl) => serde_json::to_string_pretty(&InjectedSettings::build(ctl, self.cfg.claude.wrap_statusline).json).unwrap_or_default(),
                    None => "promptly-ctl not found: nothing is injected.".into(),
                };
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    ui.label(RichText::new(json).monospace().small());
                });
            });
            ui.separator();
            ui.heading("Notifications");
            let mut c = self.core.lock();
            ui.checkbox(&mut c.notifications_enabled, "Native notifications");
            ui.checkbox(&mut c.policy.notify_on_finish, "Notify when a turn finishes");
            drop(c);
            ui.separator();
            ui.heading("Terminal");
            ui.add(egui::Slider::new(&mut self.font_size, 8.0..=32.0).text("Font size"));
            ui.checkbox(&mut self.option_as_meta, "Use Option as Meta");
            ui.checkbox(&mut self.reduce_motion, "Reduce motion (no animated numbers)");
            if ui.checkbox(&mut self.high_contrast, "High contrast").changed() {
                theme::apply_chrome(ctx, self.high_contrast);
            }
            ui.separator();
            ui.heading("Updates");
            ui.horizontal(|ui| {
                ui.label(format!("Promptly {}", current_version()));
                let status = match &*self.update.lock() {
                    UpdateState::Checking => "checking…".to_string(),
                    UpdateState::UpToDate => "up to date".to_string(),
                    UpdateState::Available(r) => format!("v{} available", r.version()),
                    UpdateState::Working(m) => m.clone(),
                    UpdateState::Ready { version, .. } => format!("v{version} ready, restart to finish"),
                    UpdateState::Failed(e) => e.clone(),
                    UpdateState::Idle => String::new(),
                };
                ui.label(RichText::new(status).color(theme::tokens::TEXT_3));
                if ui.button("Check now").clicked() {
                    actions.push(Action::CheckUpdates);
                }
            });
            ui.checkbox(&mut self.cfg.updates.check, "Check for updates automatically");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(Config::path().to_string_lossy()).monospace().small());
                if ui.button("Reload").clicked() {
                    actions.push(Action::ReloadConfig);
                }
            });
            if let Some(e) = &self.cfg_error {
                ui.colored_label(theme::RED, e);
            }
            ui.horizontal(|ui| {
                if ui.button("Hook event log").clicked() {
                    actions.push(Action::EventLog);
                }
                if ui.button("Copy diagnostics").clicked() {
                    actions.push(Action::CopyDiagnostics);
                }
            });
        });
        self.settings_open = open;
        for a in actions {
            self.run(a);
        }
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        let mut open = self.log_open;
        egui::Window::new("Hook event log")
            .open(&mut open)
            .default_size([620.0, 360.0])
            .show(ctx, |ui| {
                let c = self.core.lock();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for e in &c.log {
                            let t =
                                e.at.duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs() % 86400)
                                    .unwrap_or(0);
                            ui.label(
                                RichText::new(format!(
                                    "{:02}:{:02}:{:02}  #{:<3} {:<5} {}",
                                    t / 3600,
                                    (t / 60) % 60,
                                    t % 60,
                                    e.pane,
                                    e.kind,
                                    e.summary
                                ))
                                .monospace()
                                .small(),
                            );
                        }
                    });
            });
        self.log_open = open;
    }

    /// A reliable alternative to scrolling the TUI: the session's own transcript.
    fn transcript_window(&mut self, ctx: &egui::Context) {
        if !self.transcript_open {
            return;
        }
        let path = self.active.and_then(|id| {
            self.core
                .lock()
                .sessions
                .get(&id)
                .and_then(|m| m.transcript_path.clone())
        });
        let mut open = true;
        egui::Window::new("Session transcript")
            .open(&mut open)
            .default_size([640.0, 480.0])
            .show(ctx, |ui| {
                let Some(p) = path else {
                    ui.label("Not a Claude session.");
                    return;
                };
                let Ok(text) = std::fs::read_to_string(&p) else {
                    ui.label("Transcript not written yet.");
                    return;
                };
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in text.lines() {
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                                continue;
                            };
                            let ty = v["type"].as_str().unwrap_or("");
                            if (ty != "user" && ty != "assistant")
                                || v["isSidechain"].as_bool() == Some(true)
                            {
                                continue;
                            }
                            let content = &v["message"]["content"];
                            if let Some(t) = promptly_core::transcript::message_text(content) {
                                let col = if ty == "user" {
                                    theme::BLUE
                                } else {
                                    Color32::from_gray(0xd0)
                                };
                                ui.label(
                                    RichText::new(if ty == "user" { "You" } else { "Claude" })
                                        .small()
                                        .strong()
                                        .color(col),
                                );
                                ui.label(t);
                                ui.add_space(6.0);
                            }
                            if let Some(blocks) = content.as_array() {
                                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                                    let name = b["name"].as_str().unwrap_or("tool");
                                    ui.label(
                                        RichText::new(format!("⏺ {name}"))
                                            .small()
                                            .monospace()
                                            .color(theme::MUTED),
                                    );
                                }
                            }
                        }
                    });
            });
        self.transcript_open = open;
    }
}

thread_local! {
    static MUTE_TOGGLE: std::cell::Cell<Option<PaneId>> = const { std::cell::Cell::new(None) };
    static PRIORITY: std::cell::Cell<Option<(PaneId, i8)>> = const { std::cell::Cell::new(None) };
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Runs even while the window is hidden: PTY, control and query events
        // keep flowing without a visible frame.
        self.process_events();
        let window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        self.core.lock().focused = if window_focused { self.active } else { None };
        if self.last_poll.elapsed() > Duration::from_secs(5) {
            self.last_poll = Instant::now();
            for id in self.panes.keys().copied().collect::<Vec<_>>() {
                self.refresh_branch(id);
            }
        }
        // Reset countdowns in the usage card tick every second.
        if self.core.lock().account.limits.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        // Keep elapsed timers ticking while something is working.
        if self
            .core
            .lock()
            .sessions
            .values()
            .any(|m| m.state() == SessionState::Working)
        {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        use theme::tokens as t;
        let ctx = ui.ctx().clone();
        let overlay_open = self.palette.is_some()
            || self.history.is_some()
            || self.fanout.is_some()
            || self.paste_review.is_some();
        if !overlay_open {
            self.handle_shortcuts(&ctx);
        }
        let waiting = self.core.lock().attention.len();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(if waiting > 0 {
            format!("Promptly ({waiting})")
        } else {
            "Promptly".into()
        }));

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(264.0)
            .size_range(210.0..=380.0)
            .frame(
                egui::Frame::new()
                    .fill(t::BG_SIDEBAR)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 0,
                        bottom: 6,
                    }),
            )
            .show(ui, |ui| self.sidebar(ui));

        if self.review_open && !self.grid_mode && !self.usage_mode {
            egui::Panel::right("review")
                .resizable(true)
                .default_size(480.0)
                .size_range(300.0..=900.0)
                .frame(egui::Frame::new().fill(t::BG_SIDEBAR))
                .show(ui, |ui| {
                    let target = self.active.and_then(|id| {
                        self.core
                            .lock()
                            .sessions
                            .get(&id)
                            .map(|m| (m.cwd.clone(), m.change_seq))
                    });
                    match target {
                        Some((cwd, seq)) => {
                            self.review.sync(&cwd, seq, &ctx);
                            match self.review.show(ui) {
                                Some(ReviewAction::OpenInEditor(path, line)) => {
                                    self.open_in_editor(&path, line)
                                }
                                Some(ReviewAction::SendFeedback(text)) => {
                                    if let Some(id) = self.active {
                                        self.send_prompt(id, &text);
                                    }
                                }
                                Some(ReviewAction::Close) => self.review_open = false,
                                None => {}
                            }
                        }
                        None => {
                            ui.add_space(48.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("No session selected").color(t::TEXT_3));
                            });
                        }
                    }
                });
        }

        if self.composer_open && !self.grid_mode && !self.usage_mode && self.active.is_some() {
            egui::Panel::bottom("composer")
                .resizable(false)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(t::BG_MAIN)
                        .inner_margin(egui::Margin {
                            left: 14,
                            right: 14,
                            top: 8,
                            bottom: 12,
                        }),
                )
                .show(ui, |ui| {
                    let (cwd, target) = self
                        .active
                        .and_then(|id| {
                            self.core
                                .lock()
                                .sessions
                                .get(&id)
                                .map(|m| (m.cwd.clone(), m.display_name()))
                        })
                        .unwrap_or_else(|| (promptly_core::paths::home(), "no session".into()));
                    let send_sc = self.sc(Action::ToggleComposer);
                    let _ = send_sc;
                    match self
                        .composer
                        .show(ui, &cwd, &self.cfg.snippets.clone(), &target)
                    {
                        Some(ComposerAction::Send(text)) => {
                            if let Some(id) = self.active {
                                self.send_prompt(id, &text);
                                self.focus_terminal = true;
                            }
                        }
                        Some(ComposerAction::FocusTerminal) => self.focus_terminal = true,
                        None => {}
                    }
                });
        }

        egui::CentralPanel::no_frame().show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t::BG_MAIN);
            if self.usage_mode {
                self.usage_view(ui);
            } else if self.grid_mode {
                self.grid(ui);
            } else {
                self.panes_area(ui, overlay_open);
            }
        });

        self.overlays(&ctx);
    }

    fn on_exit(&mut self) {
        self.save_layout();
    }
}

impl App {
    fn open_in_editor(&mut self, path: &Path, line: Option<u32>) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vi".into());
        let bin = editor.split_whitespace().next().unwrap_or("vi").to_string();
        let name = Path::new(&bin)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let terminal_editor = [
            "vi", "vim", "nvim", "nano", "emacs", "hx", "helix", "micro", "kak", "mg",
        ]
        .contains(&name.as_str());
        let target = path.to_string_lossy().to_string();
        if terminal_editor {
            let line_arg = line.map(|l| format!(" +{l}")).unwrap_or_default();
            let cwd = path.parent().map(Path::to_path_buf);
            if let Some(id) = self.spawn(NewSession {
                cwd,
                split: Some(false),
                ..Default::default()
            }) && let Some(e) = self.panes.get(&id)
            {
                e.pane.write(
                    format!("{editor}{line_arg} {}\r", hooks::shell_quote(&target)).into_bytes(),
                );
            }
        } else {
            let mut cmd = std::process::Command::new("/bin/sh");
            let loc = match (line, name.as_str()) {
                (Some(l), "code" | "cursor" | "zed" | "subl") => format!("{target}:{l}"),
                _ => target,
            };
            let goto = if matches!(name.as_str(), "code" | "cursor") {
                " -g"
            } else {
                ""
            };
            cmd.arg("-c")
                .arg(format!("{editor}{goto} {}", hooks::shell_quote(&loc)));
            if let Err(e) = cmd.spawn() {
                self.toast(format!("Could not start {editor}: {e}"));
            }
        }
    }
}

#[derive(Clone)]
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(promptly_core::update::Release),
    Working(String),
    Ready {
        version: String,
        target: promptly_core::update::InstallTarget,
        exe: PathBuf,
    },
    Failed(String),
}

/// Version used for update comparison. `PROMPTLY_FAKE_VERSION` lets a test
/// build pretend to be older to exercise the update flow.
fn current_version() -> String {
    std::env::var("PROMPTLY_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

fn check_for_update(state: &Mutex<UpdateState>, ctx: &egui::Context, manual: bool) {
    // Never interrupt an install in progress.
    if matches!(
        *state.lock(),
        UpdateState::Working(_) | UpdateState::Ready { .. }
    ) {
        return;
    }
    *state.lock() = UpdateState::Checking;
    let next = match promptly_core::update::check(&current_version()) {
        Ok(Some(r)) => UpdateState::Available(r),
        Ok(None) => UpdateState::UpToDate,
        Err(e) if manual => UpdateState::Failed(format!("Could not check for updates: {e}")),
        Err(_) => UpdateState::Idle, // background checks fail quietly (offline)
    };
    *state.lock() = next;
    ctx.request_repaint();
}

fn install_update(
    state: &Mutex<UpdateState>,
    ctx: &egui::Context,
    release: promptly_core::update::Release,
) {
    use promptly_core::update;
    let set = |s: UpdateState| {
        *state.lock() = s;
        ctx.request_repaint();
    };
    let target = match update::install_target() {
        Ok(t) => t,
        Err(e) => return set(UpdateState::Failed(e)),
    };
    let work = promptly_core::paths::data_dir()
        .join("updates")
        .join(release.version());
    let progress = |m: &str| set(UpdateState::Working(m.to_string()));
    let result = update::download_and_verify(&release, &work, &progress).and_then(|staged| {
        progress("Installing…");
        update::install(&staged, &target)
    });
    let _ = std::fs::remove_dir_all(&work);
    match result {
        Ok(exe) => set(UpdateState::Ready {
            version: release.version().to_string(),
            target,
            exe,
        }),
        Err(e) => set(UpdateState::Failed(e)),
    }
}

/// Lets the user move the window by dragging empty chrome (needed with the
/// unified macOS title bar). Double-click zooms, like a native title bar.
fn window_drag_zone(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
) {
    let r = ui.interact(rect, egui::Id::new(id), egui::Sense::click_and_drag());
    if r.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if r.double_clicked() {
        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
}

/// Tinted inline notice under a pane header.
fn callout(ui: &mut egui::Ui, color: Color32, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .outer_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(body);
        });
}

/// Large borderless search input with a leading icon, used in pickers.
fn search_field(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        kit::paint_icon(ui, r, Icon::Search, theme::tokens::TEXT_3);
        let resp = ui.add(
            egui::TextEdit::singleline(text)
                .frame(egui::Frame::NONE)
                .font(egui::FontId::proportional(16.0))
                .hint_text(RichText::new(hint).size(16.0).color(theme::tokens::TEXT_3))
                .desired_width(f32::INFINITY),
        );
        resp.request_focus();
        resp
    })
    .inner
    .on_hover_cursor(egui::CursorIcon::Text)
    .union(ui.separator())
}

/// Selectable picker row with optional subtitle and shortcut keycap.
fn list_item(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    shortcut: Option<&str>,
    selected: bool,
) -> egui::Response {
    use theme::tokens as t;
    let h = if subtitle.is_some() { 46.0 } else { 34.0 };
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    if resp.hovered() && !selected {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(7), t::HOVER);
    }
    if selected {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(7), t::ACTIVE);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 8.0), egui::vec2(3.0, h - 16.0)),
            egui::CornerRadius::same(2),
            t::ACCENT_HOVER,
        );
        resp.scroll_to_me(None);
    }
    let max_w = rect.width() - 24.0 - if shortcut.is_some() { 90.0 } else { 0.0 };
    let col = if selected { t::TEXT } else { t::TEXT_1 };
    let g = kit::elide(ui, title, egui::FontId::proportional(13.5), col, max_w);
    let ty = if subtitle.is_some() {
        rect.min.y + 7.0
    } else {
        rect.center().y - g.size().y / 2.0
    };
    ui.painter()
        .galley(egui::pos2(rect.min.x + 12.0, ty), g, col);
    if let Some(sub) = subtitle {
        let g = kit::elide(ui, sub, egui::FontId::proportional(11.5), t::TEXT_3, max_w);
        ui.painter().galley(
            egui::pos2(rect.min.x + 12.0, rect.min.y + 26.0),
            g,
            t::TEXT_3,
        );
    }
    if let Some(sc) = shortcut {
        ui.painter().text(
            egui::pos2(rect.max.x - 12.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            sc,
            egui::FontId::proportional(12.0),
            t::TEXT_3,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn footer_hints(ui: &mut egui::Ui, hints: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (k, label) in hints {
            kit::kbd(ui, k);
            ui.label(
                RichText::new(*label)
                    .size(11.5)
                    .color(theme::tokens::TEXT_3),
            );
            ui.add_space(8.0);
        }
    });
}

#[allow(dead_code)]
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    }
}

fn short_path(p: &str) -> String {
    let home = promptly_core::paths::home().to_string_lossy().to_string();
    match p.strip_prefix(&home) {
        Some(rest) => format!("~{rest}"),
        None => p.to_string(),
    }
}

fn open_external(target: &str) {
    let t = if let Some(rest) = target.strip_prefix("~/") {
        promptly_core::paths::home()
            .join(rest)
            .to_string_lossy()
            .to_string()
    } else {
        target.to_string()
    };
    // Only hand off schemes we understand; never run arbitrary strings.
    let ok = ["http://", "https://", "file://", "mailto:"]
        .iter()
        .any(|s| t.starts_with(s))
        || t.starts_with('/');
    if !ok {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(&t).spawn();
}

/// Parse `primary+shift+p` style shortcuts. `primary` is Cmd on macOS and
/// Ctrl+Shift on Linux (plain Ctrl belongs to the terminal there).
pub fn parse_shortcut(spec: &str) -> Option<KeyboardShortcut> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in spec.split('+').map(|s| s.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "primary" => {
                if cfg!(target_os = "macos") {
                    m.mac_cmd = true;
                    m.command = true;
                } else {
                    if m.shift {
                        m.alt = true;
                    }
                    m.ctrl = true;
                    m.command = true;
                    m.shift = true;
                }
            }
            "cmd" | "super" => {
                m.mac_cmd = cfg!(target_os = "macos");
                m.command = true;
            }
            "ctrl" => {
                m.ctrl = true;
                if !cfg!(target_os = "macos") {
                    m.command = true;
                }
            }
            "shift" => {
                if m.shift && m.ctrl && !cfg!(target_os = "macos") {
                    m.alt = true; // primary already implies shift on Linux
                } else {
                    m.shift = true;
                }
            }
            "alt" | "option" => m.alt = true,
            k => key = parse_key(k),
        }
    }
    key.map(|k| KeyboardShortcut::new(m, k))
}

fn parse_key(k: &str) -> Option<Key> {
    Some(match k {
        "comma" | "," => Key::Comma,
        "equals" | "=" | "plus" => Key::Equals,
        "minus" | "-" => Key::Minus,
        "openbracket" | "[" => Key::OpenBracket,
        "closebracket" | "]" => Key::CloseBracket,
        "enter" => Key::Enter,
        "space" => Key::Space,
        "tab" => Key::Tab,
        other => {
            return Key::from_name(&other.to_ascii_uppercase()).or_else(|| Key::from_name(other));
        }
    })
}

fn build_shortcuts(cfg: &Config) -> Vec<(Action, KeyboardShortcut)> {
    ACTIONS
        .iter()
        .filter_map(|(a, name, _, default)| {
            let spec = cfg.binding(name).unwrap_or(default);
            parse_shortcut(spec).map(|s| (*a, s))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_parse() {
        let s = parse_shortcut("primary+shift+p").unwrap();
        assert_eq!(s.logical_key, Key::P);
        assert!(s.modifiers.shift);
        assert_eq!(
            parse_shortcut("primary+comma").unwrap().logical_key,
            Key::Comma
        );
        assert_eq!(
            parse_shortcut("primary+closebracket").unwrap().logical_key,
            Key::CloseBracket
        );
        assert!(parse_shortcut("").is_none());
        // Every default binding parses.
        for (_, name, _, d) in ACTIONS {
            if !d.is_empty() {
                assert!(parse_shortcut(d).is_some(), "{name}");
            }
        }
    }

    #[test]
    fn default_shortcuts_are_unique() {
        let cfg = Config::default();
        let s = build_shortcuts(&cfg);
        for (i, (a, x)) in s.iter().enumerate() {
            for (b, y) in &s[i + 1..] {
                assert!(
                    !(x.logical_key == y.logical_key && x.modifiers == y.modifiers),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }
}
