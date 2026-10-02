//! The prompt composer: the one place you write to Claude.
//!
//! - Enter sends, Shift+Enter adds a line, ⌃↑/⌃↓ walk history.
//! - `/` lists Claude Code's commands and every installed skill.
//! - `@` completes file paths in the repo.
//! - Attach screenshots, videos, PDFs or any file: the Attach menu, drag
//!   and drop onto the window, or a one-click paste of a clipboard image.
//!   Attachments are sent as file paths, which Claude Code reads (images
//!   and PDFs natively).

use egui::{CornerRadius, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};
use parking_lot::Mutex;
use promptly_core::commands::{self, SlashItem};
use promptly_core::nl_command::{self, Intent};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::theme::{self, tokens as t};
use crate::ui_kit::{self as kit, Icon};

const HISTORY_CAP: usize = 200;
const MAX_FILES: usize = 20_000;
const POPUP_ROWS: usize = 8;
/// Re-scan commands and skills at most this often per folder.
const SLASH_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachKind {
    Image,
    Pdf,
    Video,
    File,
}

impl AttachKind {
    pub fn of(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "tiff" => AttachKind::Image,
            "pdf" => AttachKind::Pdf,
            "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi" => AttachKind::Video,
            _ => AttachKind::File,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    pub kind: AttachKind,
    pub bytes: u64,
}

impl Attachment {
    pub fn new(path: PathBuf) -> Self {
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Self {
            kind: AttachKind::of(&path),
            path,
            bytes,
        }
    }
}

pub enum ComposerAction {
    Send {
        text: String,
        attachments: Vec<PathBuf>,
    },
    FocusTerminal,
    /// Plain-English request in a shell pane: generate a command.
    Ask {
        request: String,
    },
    /// A Claude Code command from the input's pickers (`/model sonnet`).
    Command(String),
}

/// What the popup above the composer is completing.
#[derive(Clone, PartialEq)]
enum Popup {
    Slash(Vec<SlashItem>),
    Files(String, Vec<String>),
}

/// An image waiting on the clipboard, offered as a one-click attachment.
#[derive(Clone, Copy, PartialEq)]
struct ClipImage {
    w: usize,
    h: usize,
    sig: u64,
}

/// Slash items for a (cwd, Claude config folder), and when they were read.
type SlashCache = ((PathBuf, PathBuf), Instant, Arc<Vec<SlashItem>>);

#[derive(Default)]
pub struct Composer {
    pub text: String,
    pub attachments: Vec<Attachment>,
    history: Vec<String>,
    history_pos: Option<usize>,
    files: Option<(PathBuf, Arc<Vec<String>>)>,
    slash_cache: Option<SlashCache>,
    popup_sel: usize,
    /// Text the popup was dismissed for (Esc); it reopens when the text changes.
    dismissed_for: Option<String>,
    pub focus_requested: bool,
    was_focused: bool,
    clip: Arc<Mutex<Option<ClipImage>>>,
    clip_ignored: Option<u64>,
    pub notice: Option<String>,
    /// Set by the app each frame: the target is a plain shell and
    /// plain-English requests become commands.
    pub shell_mode: bool,
    /// The target is a Claude session (shows the model and effort pickers).
    pub claude_mode: bool,
    /// The session's model and effort, for the pickers' labels.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Commands, builtins, aliases and functions the user's shell knows.
    pub known_commands: Arc<std::collections::HashSet<String>>,
    /// The user flipped Run/Ask for the current text.
    intent_flip: bool,
    /// ⌘V was down last frame (fires the image paste once per press).
    paste_latch: bool,
    /// Long pastes shown as placeholders: (placeholder, full text).
    pastes: Vec<(String, String)>,
}

pub fn edit_id() -> egui::Id {
    egui::Id::new("promptly-composer")
}

impl Composer {
    /// What Enter will do with the current text in a shell pane.
    pub fn intent(&self) -> Intent {
        let known = |w: &str| self.known_commands.contains(w);
        let guess = nl_command::classify(&self.text, &known);
        match (guess, self.intent_flip) {
            (g, false) => g,
            (Intent::Ask, true) => Intent::Command,
            (Intent::Command, true) => Intent::Ask,
        }
    }

    /// Put a command in the composer to edit; Enter will run it as typed.
    pub fn set_command(&mut self, cmd: String) {
        let known = |w: &str| self.known_commands.contains(w);
        self.intent_flip = nl_command::classify(&cmd, &known) == Intent::Ask;
        self.text = cmd;
        self.focus_requested = true;
    }

    pub fn load_history() -> Vec<String> {
        std::fs::read_to_string(history_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn with_history(history: Vec<String>) -> Self {
        Self {
            history,
            ..Default::default()
        }
    }

    fn save_history(&self) {
        if let Ok(b) = serde_json::to_vec(&self.history) {
            let p = history_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = promptly_core::util::write_private(&p, &b);
        }
    }

    /// Add files (from drag and drop or the picker), skipping duplicates.
    pub fn attach(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            if p.is_file() && !self.attachments.iter().any(|a| a.path == p) {
                self.attachments.push(Attachment::new(p));
            }
        }
        self.focus_requested = true;
    }

    /// Type into the composer from elsewhere (keys pressed in the terminal).
    pub fn insert_text(&mut self, s: &str) {
        self.text.push_str(s);
        self.focus_requested = true;
    }

    fn file_list(&mut self, cwd: &Path) -> Arc<Vec<String>> {
        if let Some((d, f)) = &self.files
            && d == cwd
        {
            return f.clone();
        }
        let list = promptly_core::util::run_stdout(
            std::process::Command::new("git").arg("-C").arg(cwd).args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
            ]),
        )
        .map(|s| {
            s.lines()
                .take(MAX_FILES)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        let list = Arc::new(list);
        self.files = Some((cwd.to_path_buf(), list.clone()));
        list
    }

    fn slash_items(&mut self, cwd: &Path, claude_dir: &Path) -> Arc<Vec<SlashItem>> {
        let key = (cwd.to_path_buf(), claude_dir.to_path_buf());
        if let Some((d, at, items)) = &self.slash_cache
            && *d == key
            && at.elapsed() < SLASH_TTL
        {
            return items.clone();
        }
        let items = Arc::new(commands::discover(cwd, claude_dir));
        self.slash_cache = Some((key, Instant::now(), items.clone()));
        items
    }

    /// The `/partial` being typed as the first word, if any.
    fn slash_query(&self) -> Option<&str> {
        let t = self.text.trim_start();
        let rest = t.strip_prefix('/')?;
        (!rest.contains(char::is_whitespace)).then_some(rest)
    }

    /// The `@partial` token at the end of the text, if any.
    fn at_token(&self) -> Option<&str> {
        let last = self.text.rsplit(|c: char| c.is_whitespace()).next()?;
        last.strip_prefix('@')
    }

    fn popup(&mut self, cwd: &Path, claude_dir: &Path) -> Option<Popup> {
        if self.dismissed_for.as_deref() == Some(self.text.as_str()) {
            return None;
        }
        // In a shell, a leading `/` is a path, not a Claude command.
        if let Some(q) = self
            .slash_query()
            .filter(|_| !self.shell_mode)
            .map(str::to_owned)
        {
            let items = self.slash_items(cwd, claude_dir);
            let m: Vec<SlashItem> = commands::filter(&items, &q)
                .into_iter()
                .take(60)
                .cloned()
                .collect();
            // Hide once the typed name is complete and unambiguous.
            if m.is_empty() || (m.len() == 1 && m[0].name == q) {
                return None;
            }
            return Some(Popup::Slash(m));
        }
        if let Some(tok) = self.at_token().map(str::to_owned) {
            let files = self.file_list(cwd);
            let q = tok.to_ascii_lowercase();
            let m: Vec<String> = files
                .iter()
                .filter(|f| fuzzy(&f.to_ascii_lowercase(), &q))
                .take(40)
                .cloned()
                .collect();
            if m.is_empty() {
                return None;
            }
            return Some(Popup::Files(tok, m));
        }
        None
    }

    fn accept(&mut self, popup: &Popup) {
        match popup {
            Popup::Slash(items) => {
                if let Some(i) = items.get(self.popup_sel) {
                    self.text = format!("/{} ", i.name);
                }
            }
            Popup::Files(tok, files) => {
                if let Some(pick) = files.get(self.popup_sel) {
                    let cut = self.text.len() - tok.len();
                    self.text.truncate(cut);
                    self.text.push_str(pick);
                    self.text.push(' ');
                }
            }
        }
        self.popup_sel = 0;
        self.focus_requested = true;
    }

    /// Look for an image on the clipboard (once per focus), off the UI thread.
    fn check_clipboard(&self, ctx: &egui::Context) {
        let slot = self.clip.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let found = arboard::Clipboard::new()
                .ok()
                .and_then(|mut c| c.get_image().ok())
                .map(|img| {
                    // Cheap signature from size and a sparse sample of pixels.
                    let b = &img.bytes;
                    let mut sig = (img.width as u64) << 32 | img.height as u64;
                    for i in (0..b.len()).step_by((b.len() / 64).max(1)) {
                        sig = sig.wrapping_mul(0x100000001b3) ^ b[i] as u64;
                    }
                    ClipImage {
                        w: img.width,
                        h: img.height,
                        sig,
                    }
                });
            *slot.lock() = found;
            ctx.request_repaint();
        });
    }

    /// ⌘V with an image on the clipboard. egui only turns ⌘V into a paste
    /// when the clipboard holds text, so a press without a text paste this
    /// frame means "paste the image". Returns true when one was attached.
    pub fn poll_image_paste(&mut self, ctx: &egui::Context) -> bool {
        let (cmd, text_paste) = ctx.input(|i| {
            (
                i.modifiers.command,
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            )
        });
        if !self.image_paste_edge(cmd && keys::v_down(), text_paste) {
            return false;
        }
        let has_image = arboard::Clipboard::new()
            .and_then(|mut c| c.get_image())
            .is_ok();
        if has_image {
            self.paste_clipboard_image();
        }
        has_image
    }

    /// True once per ⌘V press that egui didn't turn into a text paste.
    fn image_paste_edge(&mut self, down: bool, text_paste: bool) -> bool {
        let fire = down && !self.paste_latch && !text_paste;
        self.paste_latch = down;
        fire
    }

    /// Put a long paste in as a placeholder at the cursor, as Claude Code
    /// does; the full text replaces it on send.
    fn insert_paste(&mut self, ctx: &egui::Context, id: egui::Id, text: String) {
        let chars = text.chars().count();
        let mut label = format!("[Pasted {} characters text]", thousands(chars));
        if !self.pastes.is_empty() {
            label = format!(
                "[Pasted {} characters text #{}]",
                thousands(chars),
                self.pastes.len() + 1
            );
        }
        let len = self.text.chars().count();
        let mut state = egui::TextEdit::load_state(ctx, id);
        let (from, to) = state
            .as_ref()
            .and_then(|st| st.cursor.char_range())
            .map(|r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                (a.min(b).min(len), a.max(b).min(len))
            })
            .unwrap_or((len, len));
        let byte = |i: usize| {
            self.text
                .char_indices()
                .nth(i)
                .map(|(b, _)| b)
                .unwrap_or(self.text.len())
        };
        let (bf, bt) = (byte(from), byte(to));
        self.text.replace_range(bf..bt, &label);
        if let Some(st) = state.as_mut() {
            let end = egui::text::CCursor::new(from + label.chars().count());
            st.cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            st.clone().store(ctx, id);
        }
        self.pastes.push((label, text));
    }

    /// Save the clipboard image as a PNG under app data and attach it.
    fn paste_clipboard_image(&mut self) {
        let res = (|| -> Result<PathBuf, String> {
            let img = arboard::Clipboard::new()
                .and_then(|mut c| c.get_image())
                .map_err(|e| e.to_string())?;
            let dir = promptly_core::paths::data_dir().join("attachments");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let name = format!(
                "screenshot-{}.png",
                chrono::Local::now().format("%Y-%m-%d-%H%M%S")
            );
            let path = dir.join(name);
            let buf = image::RgbaImage::from_raw(
                img.width as u32,
                img.height as u32,
                img.bytes.into_owned(),
            )
            .ok_or("clipboard image has an unexpected size")?;
            buf.save(&path).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        match res {
            Ok(p) => self.attach([p]),
            Err(e) => self.notice = Some(format!("Couldn't paste the image: {e}")),
        }
        if let Some(c) = *self.clip.lock() {
            self.clip_ignored = Some(c.sig);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cwd: &Path,
        claude_dir: &Path,
        snippets: &std::collections::BTreeMap<String, String>,
        target: &str,
    ) -> Option<ComposerAction> {
        let mut action = None;
        let id = edit_id();
        let focused = ui.memory(|m| m.has_focus(id));
        if focused && !self.was_focused {
            self.check_clipboard(ui.ctx());
        }
        self.was_focused = focused;

        let popup = self.popup(cwd, claude_dir);
        let n_items = match &popup {
            Some(Popup::Slash(v)) => v.len(),
            Some(Popup::Files(_, v)) => v.len(),
            None => 0,
        };
        self.popup_sel = self.popup_sel.min(n_items.saturating_sub(1));

        // Keys handled before the TextEdit sees them.
        let (mut send, mut accept, mut esc_popup) = (false, false, false);
        let (mut up, mut down, mut hist_up, mut hist_down) = (false, false, false, false);
        if focused {
            ui.input_mut(|i| {
                if popup.is_some() {
                    up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                    down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                    accept = i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    esc_popup = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                } else {
                    send = i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
                }
                hist_up = i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp);
                hist_down = i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown);
            });
        }
        if up {
            self.popup_sel = self.popup_sel.saturating_sub(1);
        }
        if down {
            self.popup_sel = (self.popup_sel + 1).min(n_items.saturating_sub(1));
        }
        if esc_popup {
            self.dismissed_for = Some(self.text.clone());
        }
        if hist_up {
            self.history_step(true);
        }
        if hist_down {
            self.history_step(false);
        }
        if accept && let Some(p) = &popup {
            self.accept(p);
        }

        // Popup above the card.
        if let Some(p) = &popup
            && !accept
            && !esc_popup
            && let Some(picked) = self.popup_ui(ui, p)
        {
            self.popup_sel = picked;
            self.accept(p);
        }

        // Clipboard image offer.
        let clip = *self.clip.lock();
        if let Some(c) = clip.filter(|c| Some(c.sig) != self.clip_ignored) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if kit::secondary_button(
                    ui,
                    Some(Icon::Image),
                    &format!("Attach screenshot from clipboard  ·  {}×{}", c.w, c.h),
                )
                .clicked()
                {
                    self.paste_clipboard_image();
                }
                if kit::icon_button(ui, Icon::Close, "Not now", false).clicked() {
                    self.clip_ignored = Some(c.sig);
                }
            });
            ui.add_space(4.0);
        }
        if let Some(n) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(n).size(12.0).color(theme::AMBER));
                if ui.small_button("Dismiss").clicked() {
                    self.notice = None;
                }
            });
        }

        let mut text_has_focus = false;
        // Chat-style input (as on claude.ai): a rounded box with the text on
        // top and a row of controls inside it, send button at the right.
        let card = egui::Frame::new()
            .fill(t::BG_ELEVATED)
            .stroke(Stroke::new(1.0, t::BORDER_STRONG))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(egui::Margin {
                left: 16,
                right: 10,
                top: 12,
                bottom: 9,
            });
        let mut command = None;
        let card_resp = card.show(ui, |ui| {
            // Attachment chips.
            if !self.attachments.is_empty() {
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (i, a) in self.attachments.iter().enumerate() {
                        if attachment_chip(ui, a).clicked() {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    self.attachments.remove(i);
                }
                ui.add_space(8.0);
            }
            let hint = if self.shell_mode && self.attachments.is_empty() {
                "Run a command, or describe one in plain English".to_string()
            } else if self.attachments.is_empty() {
                if self.claude_mode {
                    "Reply to Claude…".to_string()
                } else {
                    format!("Message {target}…")
                }
            } else {
                "Add a message…".to_string()
            };
            let font = if self.shell_mode {
                FontId::monospace(13.5)
            } else {
                FontId::proportional(15.0)
            };
            if focused && !self.shell_mode {
                let big: Vec<String> = ui.input_mut(|i| {
                    let mut out = vec![];
                    i.events.retain(|e| match e {
                        egui::Event::Paste(t) if is_long_paste(t) => {
                            out.push(t.clone());
                            false
                        }
                        _ => true,
                    });
                    out
                });
                for t in big {
                    self.insert_paste(ui.ctx(), id, t);
                }
            }
            let edit = egui::TextEdit::multiline(&mut self.text)
                .id(id)
                .frame(egui::Frame::NONE)
                .desired_rows(if self.shell_mode { 1 } else { 2 })
                .desired_width(f32::INFINITY)
                .font(font.clone())
                .hint_text(RichText::new(hint).font(font).color(t::TEXT_3));
            let resp = ui.add(edit);
            if self.focus_requested {
                resp.request_focus();
                if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let end = egui::text::CCursor::new(self.text.chars().count());
                    st.cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    st.store(ui.ctx(), id);
                }
                self.focus_requested = false;
            }
            text_has_focus = resp.has_focus();
            if resp.changed() {
                if self.text.trim().is_empty() {
                    self.intent_flip = false;
                    self.pastes.clear();
                }
                self.dismissed_for = None;
                self.popup_sel = 0;
            }
            if text_has_focus && popup.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(ComposerAction::FocusTerminal);
            }
            ui.add_space(8.0);
            let mut send_clicked = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // "+" holds attachments and snippets.
                let plus =
                    kit::icon_button(ui, Icon::Plus, "Attach files, images or a snippet", false);
                egui::Popup::menu(&plus).show(|ui| {
                    ui.set_min_width(230.0);
                    if menu_item(ui, Icon::File, "Attach files…") {
                        ui.close();
                        if let Some(files) = rfd::FileDialog::new()
                            .set_title("Attach files")
                            .pick_files()
                        {
                            self.attach(files);
                        }
                    }
                    if menu_item(ui, Icon::Image, "Paste image from clipboard") {
                        ui.close();
                        self.paste_clipboard_image();
                    }
                    if !snippets.is_empty() {
                        ui.separator();
                        for (name, body) in snippets {
                            if menu_item(ui, Icon::Quote, name) {
                                if !self.text.is_empty() && !self.text.ends_with('\n') {
                                    self.text.push('\n');
                                }
                                self.text.push_str(body);
                                self.focus_requested = true;
                                ui.close();
                            }
                        }
                    }
                });
                if self.shell_mode {
                    if !self.text.trim().is_empty() && intent_chip(ui, self.intent()).clicked() {
                        self.intent_flip = !self.intent_flip;
                        self.focus_requested = true;
                    }
                } else if self.claude_mode {
                    ui.add_space(4.0);
                    let label = self.model.clone().unwrap_or_else(|| "Model".to_string());
                    let m = picker(ui, &label, "Model for this session (/model)");
                    egui::Popup::menu(&m).show(|ui| {
                        ui.set_min_width(220.0);
                        for (alias, name, note) in [
                            ("opus", "Opus", "Most capable"),
                            ("sonnet", "Sonnet", "Fast and capable"),
                            ("haiku", "Haiku", "Fastest"),
                        ] {
                            if option_item(ui, name, note) {
                                command = Some(format!("/model {alias}"));
                                ui.close();
                            }
                        }
                    });
                    let label = match &self.effort {
                        Some(e) => format!("Effort {e}"),
                        None => "Effort".to_string(),
                    };
                    let e = picker(ui, &label, "How hard Claude thinks (/effort)");
                    egui::Popup::menu(&e).show(|ui| {
                        ui.set_min_width(200.0);
                        for (level, note) in [
                            ("low", "Quick answers"),
                            ("medium", "Balanced"),
                            ("high", "Thinks longer"),
                            ("xhigh", "Thinks much longer"),
                            ("max", "Most thorough"),
                        ] {
                            if option_item(ui, level, note) {
                                command = Some(format!("/effort {level}"));
                                ui.close();
                            }
                        }
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = !self.text.trim().is_empty() || !self.attachments.is_empty();
                    if kit::send_button(ui, can_send).clicked() {
                        send_clicked = true;
                    }
                });
            });
            send_clicked
        });
        if let Some(c) = command {
            action = Some(ComposerAction::Command(c));
        }
        if text_has_focus {
            ui.painter().rect_stroke(
                card_resp.response.rect,
                CornerRadius::same(16),
                Stroke::new(1.0, t::TEXT_3),
                egui::StrokeKind::Inside,
            );
        }
        let send = send || card_resp.inner;
        if send && (!self.text.trim().is_empty() || !self.attachments.is_empty()) {
            let mut text = std::mem::take(&mut self.text);
            // Long pastes go out in full.
            for (label, full) in self.pastes.drain(..) {
                text = text.replacen(&label, &full, 1);
            }
            if !text.trim().is_empty() {
                self.history.retain(|h| h != &text);
                self.history.push(text.clone());
                if self.history.len() > HISTORY_CAP {
                    self.history.remove(0);
                }
                self.save_history();
            }
            self.history_pos = None;
            let ask = self.shell_mode && self.attachments.is_empty() && {
                let known = |w: &str| self.known_commands.contains(w);
                let guess = nl_command::classify(&text, &known);
                (guess == Intent::Ask) != self.intent_flip
            };
            self.intent_flip = false;
            let attachments = std::mem::take(&mut self.attachments)
                .into_iter()
                .map(|a| a.path)
                .collect();
            action = Some(if ask {
                let request = nl_command::strip_ask_prefix(&text)
                    .unwrap_or(&text)
                    .trim()
                    .to_string();
                ComposerAction::Ask { request }
            } else {
                ComposerAction::Send { text, attachments }
            });
            self.focus_requested = true;
        }
        action
    }

    /// The completion list; returns the clicked row.
    fn popup_ui(&self, ui: &mut egui::Ui, popup: &Popup) -> Option<usize> {
        let mut picked = None;
        let n = match popup {
            Popup::Slash(v) => v.len(),
            Popup::Files(_, v) => v.len(),
        };
        let first = self
            .popup_sel
            .saturating_sub(POPUP_ROWS - 1)
            .min(n.saturating_sub(POPUP_ROWS));
        egui::Frame::new()
            .fill(t::BG_ELEVATED)
            .stroke(Stroke::new(1.0, t::BORDER_STRONG))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for i in first..(first + POPUP_ROWS).min(n) {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                    let sel = i == self.popup_sel;
                    if sel {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(6), t::ACTIVE);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(6), t::HOVER);
                    }
                    let y = rect.center().y;
                    match popup {
                        Popup::Slash(items) => {
                            let it = &items[i];
                            let name = format!("/{}", it.name);
                            let g = kit::elide(
                                ui,
                                &name,
                                FontId::monospace(12.5),
                                t::TEXT,
                                rect.width() * 0.42,
                            );
                            let w = g.size().x;
                            ui.painter().galley(
                                pos2(rect.min.x + 10.0, y - g.size().y / 2.0),
                                g,
                                t::TEXT,
                            );
                            let tag = if it.is_skill {
                                format!("{} skill", it.source.label())
                            } else {
                                it.source.label().to_string()
                            };
                            let tag_col = if it.is_skill {
                                egui::Color32::from_rgb(0xa9, 0x9c, 0xf7)
                            } else {
                                t::TEXT_3
                            };
                            let tag_r = Rect::from_min_size(
                                pos2(rect.min.x + 18.0 + w, y - 9.0),
                                vec2(tag.len() as f32 * 6.2 + 12.0, 18.0),
                            );
                            ui.painter().rect_filled(
                                tag_r,
                                CornerRadius::same(9),
                                tag_col.gamma_multiply(0.14),
                            );
                            ui.painter().text(
                                tag_r.center(),
                                egui::Align2::CENTER_CENTER,
                                &tag,
                                FontId::proportional(10.5),
                                tag_col,
                            );
                            let dx = tag_r.max.x + 10.0;
                            let g = kit::elide(
                                ui,
                                &it.description,
                                FontId::proportional(12.0),
                                t::TEXT_3,
                                (rect.max.x - dx - 8.0).max(10.0),
                            );
                            ui.painter()
                                .galley(pos2(dx, y - g.size().y / 2.0), g, t::TEXT_3);
                        }
                        Popup::Files(_, files) => {
                            let g = kit::elide(
                                ui,
                                &files[i],
                                FontId::monospace(12.5),
                                t::TEXT_1,
                                rect.width() - 20.0,
                            );
                            ui.painter().galley(
                                pos2(rect.min.x + 10.0, y - g.size().y / 2.0),
                                g,
                                t::TEXT_1,
                            );
                        }
                    }
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
                let hint = match popup {
                    Popup::Slash(_) => {
                        format!("{n} commands and skills · ↑↓ choose · ↵ or Tab insert · Esc close")
                    }
                    Popup::Files(..) => "↑↓ choose · ↵ or Tab insert · Esc close".to_string(),
                };
                ui.label(RichText::new(hint).size(11.0).color(t::TEXT_3));
            });
        ui.add_space(4.0);
        picked
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let n = self.history.len();
        let pos = match (self.history_pos, back) {
            (None, true) => n - 1,
            (None, false) => return,
            (Some(p), true) => p.saturating_sub(1),
            (Some(p), false) if p + 1 >= n => {
                self.history_pos = None;
                self.text.clear();
                return;
            }
            (Some(p), false) => p + 1,
        };
        self.history_pos = Some(pos);
        self.text = self.history[pos].clone();
    }
}

/// A removable attachment chip: kind icon, file name, size.
fn attachment_chip(ui: &mut egui::Ui, a: &Attachment) -> egui::Response {
    let name = a
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let font = FontId::proportional(12.0);
    let size = human_size(a.bytes);
    let name_w = ui
        .fonts_mut(|f| f.layout_no_wrap(name.clone(), font.clone(), t::TEXT_1))
        .size()
        .x
        .min(240.0);
    let size_w = ui
        .fonts_mut(|f| f.layout_no_wrap(size.clone(), FontId::proportional(11.0), t::TEXT_3))
        .size()
        .x;
    let w = 30.0 + name_w + 8.0 + size_w + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    let h = kit::hover_t(ui, &resp);
    let (icon, col) = match a.kind {
        AttachKind::Image => (Icon::Image, theme::BLUE),
        AttachKind::Pdf => (Icon::FileText, theme::RED),
        AttachKind::Video => (Icon::Film, theme::AMBER),
        AttachKind::File => (Icon::File, t::TEXT_2),
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        kit::mix(t::BG_ELEVATED, t::BG_ELEVATED_2, h),
        Stroke::new(1.0, t::BORDER),
        egui::StrokeKind::Inside,
    );
    kit::paint_icon(
        ui,
        egui::Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(14.0, 14.0)),
        icon,
        col,
    );
    let g = kit::elide(ui, &name, font, t::TEXT_1, name_w);
    ui.painter().galley(
        pos2(rect.min.x + 30.0, rect.center().y - g.size().y / 2.0),
        g,
        t::TEXT_1,
    );
    ui.painter().text(
        pos2(rect.min.x + 30.0 + name_w + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        size,
        FontId::proportional(11.0),
        t::TEXT_3,
    );
    kit::paint_icon(
        ui,
        egui::Rect::from_center_size(pos2(rect.max.x - 15.0, rect.center().y), vec2(12.0, 12.0)),
        Icon::Close,
        kit::mix(t::TEXT_3, t::TEXT, h),
    );
    let hover = match a.kind {
        AttachKind::Video => {
            "Claude Code can't watch video directly; it gets the file path and can use tools (like ffmpeg) to inspect it. Click to remove."
        }
        _ => "Click to remove",
    };
    resp.on_hover_text(format!("{}\n{hover}", a.path.display()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Shows what Enter does in a shell pane; click to switch.
fn intent_chip(ui: &mut egui::Ui, intent: Intent) -> egui::Response {
    let (icon, label, col) = match intent {
        Intent::Ask => (Icon::Sparkle, "Ask Claude for a command", t::ACCENT_HOVER),
        Intent::Command => (Icon::Terminal, "Run in shell", t::TEXT_2),
    };
    let font = FontId::proportional(11.5);
    let w = ui
        .fonts_mut(|f| f.layout_no_wrap(label.into(), font.clone(), col))
        .size()
        .x
        + 32.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    let h = kit::hover_t(ui, &resp);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(12),
        kit::mix(col.gamma_multiply(0.10), col.gamma_multiply(0.18), h),
    );
    kit::paint_icon(
        ui,
        egui::Rect::from_center_size(pos2(rect.min.x + 14.0, rect.center().y), vec2(12.0, 12.0)),
        icon,
        col,
    );
    ui.painter().text(
        pos2(rect.min.x + 25.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        col,
    );
    resp.on_hover_text("Click to switch. Start with # to always ask Claude.")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pastes this long become a placeholder (Claude Code's behaviour).
fn is_long_paste(t: &str) -> bool {
    t.chars().count() > 500 || t.matches('\n').count() >= 2
}

/// 1234 -> "1,234".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Whether the V key is physically down (macOS), for image paste.
mod keys {
    #[cfg(target_os = "macos")]
    pub fn v_down() -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
        }
        // kCGEventSourceStateCombinedSessionState, kVK_ANSI_V.
        // SAFETY: a pure query with no pointers.
        unsafe { CGEventSourceKeyState(0, 0x09) }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn v_down() -> bool {
        false
    }
}

/// Small text button with a chevron that opens a menu.
fn picker(ui: &mut egui::Ui, label: &str, tip: &str) -> egui::Response {
    let font = FontId::proportional(12.5);
    let w = ui
        .fonts_mut(|f| f.layout_no_wrap(label.into(), font.clone(), t::TEXT_2))
        .size()
        .x
        + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    let h = kit::hover_t(ui, &resp);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(7),
        kit::mix(egui::Color32::TRANSPARENT, t::HOVER, h),
    );
    let col = kit::mix(t::TEXT_2, t::TEXT, h);
    ui.painter().text(
        pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        col,
    );
    kit::paint_icon(
        ui,
        egui::Rect::from_center_size(pos2(rect.max.x - 12.0, rect.center().y), vec2(11.0, 11.0)),
        Icon::ChevronDown,
        col,
    );
    resp.on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Menu row: a name and a quiet description.
fn option_item(ui: &mut egui::Ui, name: &str, note: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let h = kit::hover_t(ui, &resp);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        kit::mix(egui::Color32::TRANSPARENT, t::HOVER, h),
    );
    ui.painter().text(
        pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.0),
        t::TEXT,
    );
    ui.painter().text(
        pos2(rect.max.x - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        note,
        FontId::proportional(11.5),
        t::TEXT_3,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// Popup menu row with a leading icon.
fn menu_item(ui: &mut egui::Ui, icon: Icon, label: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let h = kit::hover_t(ui, &resp);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        kit::mix(egui::Color32::TRANSPARENT, t::HOVER, h),
    );
    kit::paint_icon(
        ui,
        egui::Rect::from_center_size(pos2(rect.min.x + 15.0, rect.center().y), vec2(14.0, 14.0)),
        icon,
        t::TEXT_2,
    );
    ui.painter().text(
        pos2(rect.min.x + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        t::TEXT,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn human_size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Paths as Claude Code expects them when dropped into its input: spaces
/// and shell metacharacters escaped with a backslash.
pub fn escape_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if " ()[]{}'\"&;$`!*?<>|\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Subsequence match, so `@srcmain` finds `src/main.rs`.
pub fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|n| it.any(|h| h == n))
}

fn history_path() -> PathBuf {
    promptly_core::paths::data_dir().join("composer-history.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paste_fires_once_per_press_without_text() {
        let mut c = Composer::default();
        assert!(c.image_paste_edge(true, false), "press with no text paste");
        assert!(!c.image_paste_edge(true, false), "held: no repeat");
        assert!(!c.image_paste_edge(false, false));
        assert!(!c.image_paste_edge(true, true), "text paste wins");
        assert!(!c.image_paste_edge(false, false));
        assert!(c.image_paste_edge(true, false));
        // Linking and calling the macOS key query works (V isn't held now).
        let _ = keys::v_down();
    }

    #[test]
    fn long_paste_becomes_placeholder_and_sends_in_full() {
        use egui_kittest::Harness;
        struct S {
            c: Composer,
            sent: Option<String>,
            fonts: bool,
        }
        let long = "fn main() {\n    println!(\"hi\");\n}\n".repeat(30);
        let snippets = std::collections::BTreeMap::new();
        let mut h = Harness::builder()
            .with_size(egui::vec2(800.0, 300.0))
            .build_ui_state(
                move |ui, st: &mut S| {
                    // Fonts apply from the next frame: draw nothing until then.
                    if !st.fonts {
                        crate::fonts::install(ui.ctx());
                        st.fonts = true;
                        return;
                    }
                    if let Some(ComposerAction::Send { text, .. }) = st.c.show(
                        ui,
                        Path::new("/tmp"),
                        Path::new("/tmp"),
                        &snippets,
                        "Claude",
                    ) {
                        st.sent = Some(text);
                    }
                },
                S {
                    c: Composer {
                        claude_mode: true,
                        focus_requested: true,
                        text: "Look at this: ".into(),
                        ..Default::default()
                    },
                    sent: None,
                    fonts: false,
                },
            );
        h.run();
        h.input_mut().events.push(egui::Event::Paste(long.clone()));
        h.run();
        let label = format!(
            "[Pasted {} characters text]",
            thousands(long.chars().count())
        );
        assert_eq!(h.state().c.text, format!("Look at this: {label}"));
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(
            h.state().sent.as_deref(),
            Some(format!("Look at this: {long}").as_str())
        );
        // Short pastes go in as typed.
        h.input_mut().events.push(egui::Event::Paste("abc".into()));
        h.run();
        assert_eq!(h.state().c.text, "abc");
    }

    #[test]
    fn long_pastes_collapse() {
        assert!(!is_long_paste("short one-liner"));
        assert!(!is_long_paste("two\nlines"));
        assert!(is_long_paste("a\nb\nc"));
        assert!(is_long_paste(&"x".repeat(501)));
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy("src/main.rs", "srcmain"));
        assert!(fuzzy("src/main.rs", ""));
        assert!(!fuzzy("src/main.rs", "zz"));
    }

    #[test]
    fn tokens() {
        let mut c = Composer {
            text: "look at @src/ma".into(),
            ..Default::default()
        };
        assert_eq!(c.at_token(), Some("src/ma"));
        c.text = "email me@x.com please".into();
        assert_eq!(c.at_token(), None);
        c.text = "/comp".into();
        assert_eq!(c.slash_query(), Some("comp"));
        c.text = "/compact now".into();
        assert_eq!(c.slash_query(), None, "only while typing the command name");
        c.text = "a/b".into();
        assert_eq!(c.slash_query(), None);
    }

    #[test]
    fn attachment_kinds_and_escaping() {
        assert_eq!(AttachKind::of(Path::new("a/Shot 1.PNG")), AttachKind::Image);
        assert_eq!(AttachKind::of(Path::new("spec.pdf")), AttachKind::Pdf);
        assert_eq!(AttachKind::of(Path::new("demo.mov")), AttachKind::Video);
        assert_eq!(AttachKind::of(Path::new("notes.txt")), AttachKind::File);
        assert_eq!(
            escape_path(Path::new("/tmp/Screen Shot (1).png")),
            "/tmp/Screen\\ Shot\\ \\(1\\).png"
        );
    }
}
