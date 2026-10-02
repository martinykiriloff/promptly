//! Session model shared between the UI and background threads.
//!
//! Hook events, transcript updates and notifications are processed on
//! background threads under the `Core` lock, so a minimized or occluded
//! window never delays a permission alert.

use parking_lot::Mutex;
use promptly_core::accounts::{self, Account, AccountsFile};
use promptly_core::attention::{AttentionItem, AttentionQueue, NotifyPolicy};
use promptly_core::git::Worktree;
use promptly_core::hooks::HookEvent;
use promptly_core::ipc::{Incoming, Server};
use promptly_core::session_changes;
use promptly_core::state::{SessionState, StateTracker, Transition};
use promptly_core::statusline::StatusInfo;
use promptly_core::transcript::{TranscriptReader, TranscriptStats, TranscriptWatch};
use promptly_core::usage::AccountUsage;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant, SystemTime};

use crate::events::AppEvent;
use crate::pty::PaneId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneKind {
    Shell,
    Claude { session_id: String },
}

pub struct SessionMeta {
    pub kind: PaneKind,
    /// Id of the Claude account (config folder) this session runs under.
    pub account: String,
    pub title: String,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub tracker: StateTracker,
    pub stats: TranscriptStats,
    pub status: StatusInfo,
    pub started: Instant,
    pub muted: bool,
    pub priority: i8,
    pub worktree: Option<Worktree>,
    pub last_exit: Option<i32>,
    pub command_running: bool,
    pub hooks_injected: bool,
    pub transcript_path: Option<PathBuf>,
    pub attention_detail: Option<String>,
    /// OSC 52 write permission, asked once per session.
    pub clipboard_allowed: Option<bool>,
    pub pending_clipboard: Option<String>,
    /// Bumped whenever files may have changed, so the review pane refreshes.
    pub change_seq: u64,
    /// Effort level picked in Promptly (Claude Code doesn't report it).
    pub effort: Option<String>,
    /// Each file the agent edited, as it was before the first edit.
    pub baselines: Arc<session_changes::Baselines>,
    /// Cumulative cost (USD) and tokens over time, for burn rates and sparklines.
    pub cost_series: promptly_core::usage::Series,
    pub token_series: promptly_core::usage::Series,
    /// Claude's process, newest last: prompts, thinking, tool calls, replies.
    pub steps: VecDeque<promptly_core::transcript::Step>,
}

impl SessionMeta {
    pub fn new(kind: PaneKind, cwd: PathBuf) -> Self {
        let mut tracker = StateTracker::new();
        if kind == PaneKind::Shell {
            // Plain shells have no agent state; show them as idle.
            tracker.on_transcript(promptly_core::transcript::TranscriptActivity::TurnEnded);
        }
        Self {
            kind,
            account: String::new(),
            title: String::new(),
            cwd,
            branch: None,
            tracker,
            stats: TranscriptStats::default(),
            status: StatusInfo::default(),
            started: Instant::now(),
            muted: false,
            priority: 0,
            worktree: None,
            last_exit: None,
            command_running: false,
            hooks_injected: false,
            transcript_path: None,
            attention_detail: None,
            clipboard_allowed: None,
            pending_clipboard: None,
            change_seq: 0,
            effort: None,
            baselines: Arc::default(),
            cost_series: promptly_core::usage::Series::with_cap(2000),
            token_series: promptly_core::usage::Series::with_cap(2000),
            steps: VecDeque::new(),
        }
    }

    pub fn is_claude(&self) -> bool {
        matches!(self.kind, PaneKind::Claude { .. })
    }

    pub fn state(&self) -> SessionState {
        self.tracker.state()
    }

    pub fn context_percent(&self) -> Option<f32> {
        self.status
            .context_percent
            .or_else(|| self.stats.context_percent())
    }

    pub fn display_name(&self) -> String {
        // Claude Code prefixes its title with a spinner glyph ("✳ Fix bug");
        // the sidebar already shows state, so drop leading symbols.
        let title = self
            .title
            .trim_start_matches(|c: char| !(c.is_alphanumeric() || "~/.([@".contains(c)))
            .trim();
        if !title.is_empty() {
            return title.to_string();
        }
        if let Some(p) = &self.stats.first_prompt {
            return promptly_core::sanitize::one_line(p, 40);
        }
        self.cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "~".into())
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: SystemTime,
    pub pane: PaneId,
    pub kind: &'static str,
    pub summary: String,
}

pub struct Core {
    pub sessions: BTreeMap<PaneId, SessionMeta>,
    pub attention: AttentionQueue,
    pub policy: NotifyPolicy,
    pub notifications_enabled: bool,
    /// The pane the user is looking at, if the window has focus.
    pub focused: Option<PaneId>,
    pub log: VecDeque<LogEntry>,
    /// Claude accounts on the machine (one per config folder).
    pub accounts: Vec<Account>,
    /// The account new sessions use.
    pub active_account: String,
    accounts_file: AccountsFile,
    /// Plan limits reported by Claude Code, per account id.
    pub usage: HashMap<String, AccountUsage>,
    /// Today's tokens across every Claude session on the machine.
    pub daily: Option<promptly_core::usage::DailyUsage>,
}

const LOG_CAP: usize = 2000;
/// Steps kept per session for the thinking panel.
const MAX_STEPS: usize = 400;

impl Core {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            attention: AttentionQueue::default(),
            policy: NotifyPolicy::default(),
            notifications_enabled: true,
            focused: None,
            log: VecDeque::new(),
            accounts: vec![],
            active_account: String::new(),
            accounts_file: AccountsFile::load(),
            usage: HashMap::new(),
            daily: None,
        }
        .with_accounts()
    }

    fn with_accounts(mut self) -> Self {
        self.refresh_accounts();
        let inherited = crate::INHERITED_CONFIG_DIR.get();
        self.active_account = self
            .accounts_file
            .active
            .clone()
            .filter(|id| self.account(id).is_some())
            .or_else(|| {
                let d = inherited?;
                self.accounts
                    .iter()
                    .find(|a| &a.dir == d)
                    .map(|a| a.id.clone())
            })
            .or_else(|| self.accounts.first().map(|a| a.id.clone()))
            .unwrap_or_default();
        self
    }

    /// Re-scan account folders and identities (e.g. after a `/login`).
    pub fn refresh_accounts(&mut self) {
        let inherited = crate::INHERITED_CONFIG_DIR.get().map(PathBuf::as_path);
        self.accounts = accounts::discover(
            &promptly_core::paths::home(),
            &self.accounts_file,
            inherited,
        );
        for a in &self.accounts {
            self.usage
                .entry(a.id.clone())
                .or_insert_with(|| AccountUsage::load(a));
        }
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// The account new sessions use.
    pub fn active(&self) -> &Account {
        self.account(&self.active_account)
            .or(self.accounts.first())
            .expect("~/.claude is always an account")
    }

    pub fn set_active(&mut self, id: &str) {
        if self.account(id).is_some() {
            self.active_account = id.to_string();
            self.accounts_file.active = Some(id.to_string());
            self.accounts_file.save();
        }
    }

    pub fn rename_account(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.accounts_file.names.remove(id);
        } else {
            self.accounts_file.names.insert(id.into(), name.into());
        }
        self.accounts_file.save();
        self.refresh_accounts();
    }

    /// Position of an account in the list (drives its avatar colour).
    pub fn account_index(&self, id: &str) -> usize {
        self.accounts.iter().position(|a| a.id == id).unwrap_or(0)
    }

    pub fn multi_account(&self) -> bool {
        self.accounts.len() > 1
    }

    /// Plan limits for an account (empty until Claude reports them).
    pub fn usage_of(&self, id: &str) -> &AccountUsage {
        static EMPTY: std::sync::LazyLock<AccountUsage> =
            std::sync::LazyLock::new(Default::default);
        self.usage.get(id).unwrap_or(&EMPTY)
    }

    /// Short account tag for a session, when there is more than one account.
    pub fn account_tag(&self, id: &str) -> Option<String> {
        self.multi_account()
            .then(|| self.account(id).map(Account::short_label))
            .flatten()
    }

    pub fn log(&mut self, pane: PaneId, kind: &'static str, summary: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogEntry {
            at: SystemTime::now(),
            pane,
            kind,
            summary,
        });
    }

    /// Update the attention queue and decide on a notification.
    /// Returns `(title, body)` when one should be shown.
    pub fn apply(&mut self, pane: PaneId, tr: Option<Transition>) -> Option<(String, String)> {
        let tr = tr?;
        let focused = self.focused == Some(pane);
        let meta = self.sessions.get_mut(&pane)?;
        meta.attention_detail = tr.detail.clone();
        let (muted, priority, since, name) = (
            meta.muted,
            meta.priority,
            meta.tracker.since(),
            meta.display_name(),
        );
        let is_claude = meta.is_claude();
        self.attention.update(AttentionItem {
            session: pane,
            state: tr.to,
            since,
            detail: tr.detail.clone(),
            priority,
        });
        self.log(
            pane,
            "state",
            format!("{} -> {}", tr.from.label(), tr.to.label()),
        );
        // Plain shells only notify on a non-zero exit.
        if !is_claude && tr.to != SessionState::Error {
            return None;
        }
        let worked = tr.from == SessionState::Working;
        let finish = matches!(tr.to, SessionState::Done | SessionState::Idle);
        if !self.notifications_enabled || (finish && !worked) {
            return None;
        }
        if !self.policy.should_notify(pane, tr.to, muted, focused) {
            return None;
        }
        let body = match tr.to {
            SessionState::WaitingPermission => {
                format!(
                    "Needs permission: {}",
                    tr.detail.as_deref().unwrap_or("tool call")
                )
            }
            SessionState::WaitingInput => "Waiting for your input".into(),
            SessionState::Done => "Finished: files changed and tests passed".into(),
            SessionState::Idle => "Turn finished".into(),
            SessionState::Error => {
                format!("Exited with {}", tr.detail.as_deref().unwrap_or("an error"))
            }
            _ => return None,
        };
        Some((name, body))
    }
}

pub type SharedCore = Arc<Mutex<Core>>;

/// Background machinery for one Claude session: its hook socket and
/// transcript tail thread. Dropping it stops both.
pub struct ClaudeLink {
    _server: Server,
    stop: Arc<AtomicBool>,
    pub settings_path: PathBuf,
}

impl Drop for ClaudeLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.settings_path);
    }
}

pub struct LinkCtx {
    pub core: SharedCore,
    pub tx: Sender<AppEvent>,
    pub ctx: egui::Context,
}

impl ClaudeLink {
    pub fn start(
        pane: PaneId,
        socket: PathBuf,
        token: String,
        settings_path: PathBuf,
        initial_transcript: PathBuf,
        lc: LinkCtx,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (path_tx, path_rx) = mpsc::channel::<PathBuf>();
        let path_tx = Mutex::new(path_tx);

        let core = lc.core.clone();
        let tx = lc.tx.clone();
        let ctx = lc.ctx.clone();
        let seen = Mutex::new(std::collections::HashSet::<PathBuf>::new());
        let server = Server::spawn(socket, Some(token), move |msg| {
            // Copy a file just before Claude first edits it, before taking the
            // core lock: the hook client waits for this, so the copy is
            // always of the file as it was.
            let baseline = match &msg {
                Incoming::Hook(v) => HookEvent::parse(v)
                    .filter(|e| e.hook_event_name == "PreToolUse")
                    .and_then(|e| session_changes::edited_path(&e))
                    .filter(|p| seen.lock().insert(p.clone()))
                    .map(|p| {
                        let b = session_changes::capture(&p);
                        (p, b)
                    }),
                _ => None,
            };
            let note = {
                let mut c = core.lock();
                match msg {
                    Incoming::Hook(v) => {
                        let Some(ev) = HookEvent::parse(&v) else {
                            return;
                        };
                        let summary = match ev.tool_summary() {
                            Some(t) => format!("{} {t}", ev.hook_event_name),
                            None => ev.hook_event_name.clone(),
                        };
                        c.log(pane, "hook", summary);
                        let Some(meta) = c.sessions.get_mut(&pane) else {
                            return;
                        };
                        if let Some((path, base)) = baseline {
                            Arc::make_mut(&mut meta.baselines)
                                .entry(path)
                                .or_insert(base);
                            meta.change_seq += 1;
                        }
                        // Claude reports its working directory with every hook.
                        if let Some(cwd) = ev.cwd.as_ref().filter(|c| c.is_dir())
                            && meta.cwd != *cwd
                        {
                            meta.cwd = cwd.clone();
                            meta.change_seq += 1;
                        }
                        if let Some(p) = &ev.transcript_path
                            && meta.transcript_path.as_ref() != Some(p)
                        {
                            meta.transcript_path = Some(p.clone());
                            let _ = path_tx.lock().send(p.clone());
                        }
                        if ev.is_file_change() || ev.hook_event_name == "Stop" {
                            meta.change_seq += 1;
                        }
                        let tr = meta.tracker.on_hook(&ev);
                        c.apply(pane, tr)
                    }
                    Incoming::Statusline(v) => {
                        let info = StatusInfo::parse(&v);
                        let now = promptly_core::usage::now_secs();
                        let limits = info.rate_limits;
                        let account = c.sessions.get(&pane).map(|m| m.account.clone());
                        if let Some(meta) = c.sessions.get_mut(&pane) {
                            if let Some(cost) = info.cost_usd {
                                meta.cost_series.push(now as f64, cost);
                            }
                            if let (Some(i), Some(o)) = (info.input_tokens, info.output_tokens) {
                                meta.token_series.push(now as f64, (i + o) as f64);
                            }
                            meta.status = info;
                        }
                        match (limits, account.and_then(|id| c.account(&id).cloned())) {
                            (Some(r), Some(acct)) => {
                                let tag = c.account_tag(&acct.id);
                                let u = c
                                    .usage
                                    .entry(acct.id.clone())
                                    .or_insert_with(|| AccountUsage::load(&acct));
                                let alerts = u.observe(r, now);
                                u.save(&acct);
                                alerts.first().map(|a| {
                                    (
                                        match &tag {
                                            Some(t) => {
                                                format!("{t}: {} limit at {:.0}%", a.window, a.used)
                                            }
                                            None => format!(
                                                "{} limit at {:.0}%",
                                                capitalize(a.window),
                                                a.used
                                            ),
                                        },
                                        format!(
                                            "Crossed {}%. Resets in {}.",
                                            a.threshold,
                                            promptly_core::usage::fmt_until(a.resets_at - now)
                                        ),
                                    )
                                })
                            }
                            _ => None,
                        }
                    }
                    Incoming::Control(..) => None,
                }
            };
            if let Some((title, body)) = note {
                crate::notify::send(title, body, Some(pane), tx.clone(), ctx.clone());
            }
            ctx.request_repaint();
        })?;

        let stop2 = stop.clone();
        let core = lc.core.clone();
        let tx = lc.tx;
        let ctx = lc.ctx;
        std::thread::Builder::new()
            .name(format!("transcript {pane}"))
            .spawn(move || {
                let mut path = initial_transcript;
                'outer: while !stop2.load(Ordering::Relaxed) {
                    let mut reader = TranscriptReader::new(path.clone());
                    let watch = TranscriptWatch::new(&path).ok();
                    loop {
                        if stop2.load(Ordering::Relaxed) {
                            break 'outer;
                        }
                        if let Ok(p) = path_rx.try_recv() {
                            path = p;
                            continue 'outer;
                        }
                        let activity = reader.poll().ok().flatten();
                        let got_steps: bool;
                        let note = {
                            let mut c = core.lock();
                            let Some(meta) = c.sessions.get_mut(&pane) else {
                                break 'outer;
                            };
                            if meta.stats != reader.stats {
                                meta.stats = reader.stats.clone();
                            }
                            let new_steps = reader.take_steps();
                            got_steps = !new_steps.is_empty();
                            meta.steps.extend(new_steps);
                            while meta.steps.len() > MAX_STEPS {
                                meta.steps.pop_front();
                            }
                            let tr = activity.and_then(|a| meta.tracker.on_transcript(a));
                            c.apply(pane, tr)
                        };
                        if let Some((title, body)) = note {
                            crate::notify::send(title, body, Some(pane), tx.clone(), ctx.clone());
                        }
                        if activity.is_some() || got_steps {
                            ctx.request_repaint();
                        }
                        match &watch {
                            Some(w) => {
                                w.wait(Duration::from_millis(1000));
                            }
                            None => std::thread::sleep(Duration::from_millis(500)),
                        }
                    }
                }
            })?;
        Ok(Self {
            _server: server,
            stop,
            settings_path,
        })
    }
}

/// What gets saved so layout and sessions survive a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPane {
    pub kind: PaneKind,
    /// Account id; older layouts have none and use the active account.
    #[serde(default)]
    pub account: Option<String>,
    pub cwd: PathBuf,
    pub title: String,
    pub worktree: Option<(PathBuf, String, PathBuf)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedLayout {
    pub tabs: Vec<Vec<SavedPane>>,
    pub review_open: bool,
    pub composer_open: bool,
}

impl SavedLayout {
    fn path() -> PathBuf {
        promptly_core::paths::data_dir().join("layout.json")
    }

    pub fn load() -> Option<Self> {
        serde_json::from_slice(&std::fs::read(Self::path()).ok()?).ok()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = promptly_core::util::write_private(&p, &b);
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}
