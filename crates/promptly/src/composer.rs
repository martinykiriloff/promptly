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
    fn label(self) -> &'static str {
        match self {
            AttachKind::Image => "Image",
            AttachKind::Pdf => "PDF",
            AttachKind::Video => "Video",
            AttachKind::File => "File",
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

#[derive(Default)]
pub struct Composer {
    pub text: String,
    pub attachments: Vec<Attachment>,
    history: Vec<String>,
    history_pos: Option<usize>,
    files: Option<(PathBuf, Arc<Vec<String>>)>,
    slash_cache: Option<(PathBuf, Instant, Arc<Vec<SlashItem>>)>,
    popup_sel: usize,
    /// Text the popup was dismissed for (Esc); it reopens when the text changes.
    dismissed_for: Option<String>,
    pub focus_requested: bool,
    was_focused: bool,
    clip: Arc<Mutex<Option<ClipImage>>>,
    clip_ignored: Option<u64>,
    pub notice: Option<String>,
}

pub fn edit_id() -> egui::Id {
    egui::Id::new("promptly-composer")
}

impl Composer {
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

    fn slash_items(&mut self, cwd: &Path) -> Arc<Vec<SlashItem>> {
        if let Some((d, at, items)) = &self.slash_cache
            && d == cwd
            && at.elapsed() < SLASH_TTL
        {
            return items.clone();
        }
        let items = Arc::new(commands::discover(cwd, &promptly_core::paths::claude_dir()));
        self.slash_cache = Some((cwd.to_path_buf(), Instant::now(), items.clone()));
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

    fn popup(&mut self, cwd: &Path) -> Option<Popup> {
        if self.dismissed_for.as_deref() == Some(self.text.as_str()) {
            return None;
        }
        if let Some(q) = self.slash_query().map(str::to_owned) {
            let items = self.slash_items(cwd);
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

        let popup = self.popup(cwd);
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
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!(
                                "Paste screenshot from clipboard ({}×{})",
                                c.w, c.h
                            ))
                            .size(12.0),
                        )
                        .corner_radius(CornerRadius::same(12)),
                    )
                    .clicked()
                {
                    self.paste_clipboard_image();
                }
                if ui.small_button("Not now").clicked() {
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
        let card = egui::Frame::new()
            .fill(t::BG_INPUT)
            .stroke(Stroke::new(1.0, t::BORDER))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin {
                left: 12,
                right: 8,
                top: 10,
                bottom: 8,
            });
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
                ui.add_space(6.0);
            }
            let hint = if self.attachments.is_empty() {
                format!("Message {target}…  / for commands and skills, @ for files")
            } else {
                "Add a message, or press Enter to send the attachments".to_string()
            };
            let edit = egui::TextEdit::multiline(&mut self.text)
                .id(id)
                .frame(egui::Frame::NONE)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .font(FontId::proportional(14.0))
                .hint_text(RichText::new(hint).size(14.0).color(t::TEXT_3));
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
                self.dismissed_for = None;
                self.popup_sel = 0;
            }
            if text_has_focus && popup.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(ComposerAction::FocusTerminal);
            }
            ui.add_space(4.0);
            let mut send_clicked = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.menu_button(RichText::new("Attach").size(12.0).color(t::TEXT_2), |ui| {
                    if ui.button("Choose files…").clicked() {
                        ui.close();
                        if let Some(files) = rfd::FileDialog::new()
                            .set_title("Attach files")
                            .pick_files()
                        {
                            self.attach(files);
                        }
                    }
                    if ui.button("Paste image from clipboard").clicked() {
                        ui.close();
                        self.paste_clipboard_image();
                    }
                    ui.separator();
                    ui.label(
                        RichText::new("Or drag files onto the window")
                            .size(11.5)
                            .color(t::TEXT_3),
                    );
                });
                ui.menu_button(
                    RichText::new("Snippets").size(12.0).color(t::TEXT_2),
                    |ui| {
                        for (name, body) in snippets {
                            if ui.button(name).on_hover_text(body).clicked() {
                                if !self.text.is_empty() && !self.text.ends_with('\n') {
                                    self.text.push('\n');
                                }
                                self.text.push_str(body);
                                self.focus_requested = true;
                                ui.close();
                            }
                        }
                    },
                );
                ui.label(
                    RichText::new("⇧↵ new line  ⌃↑↓ history")
                        .size(11.0)
                        .color(t::TEXT_3),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = !self.text.trim().is_empty() || !self.attachments.is_empty();
                    ui.add_enabled_ui(can_send, |ui| {
                        if kit::primary_button(ui, Some(Icon::Send), "Send", Some("↵"), false)
                            .clicked()
                        {
                            send_clicked = true;
                        }
                    });
                });
            });
            send_clicked
        });
        if text_has_focus {
            ui.painter().rect_stroke(
                card_resp.response.rect,
                CornerRadius::same(10),
                Stroke::new(1.0, t::ACCENT.gamma_multiply(0.8)),
                egui::StrokeKind::Inside,
            );
        }
        let send = send || card_resp.inner;
        if send && (!self.text.trim().is_empty() || !self.attachments.is_empty()) {
            let text = std::mem::take(&mut self.text);
            if !text.trim().is_empty() {
                self.history.retain(|h| h != &text);
                self.history.push(text.clone());
                if self.history.len() > HISTORY_CAP {
                    self.history.remove(0);
                }
                self.save_history();
            }
            self.history_pos = None;
            let attachments = std::mem::take(&mut self.attachments)
                .into_iter()
                .map(|a| a.path)
                .collect();
            action = Some(ComposerAction::Send { text, attachments });
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

/// A removable attachment chip: kind, file name, size.
fn attachment_chip(ui: &mut egui::Ui, a: &Attachment) -> egui::Response {
    let name = a
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let label = format!("{}  {}  {}", a.kind.label(), name, human_size(a.bytes));
    let font = FontId::proportional(12.0);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), t::TEXT_1));
    let w = g.size().x.min(320.0) + 40.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let col = match a.kind {
        AttachKind::Image => theme::BLUE,
        AttachKind::Pdf => theme::RED,
        AttachKind::Video => theme::AMBER,
        AttachKind::File => t::TEXT_2,
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(13),
        col.gamma_multiply(0.12),
        Stroke::new(1.0, col.gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );
    let g = kit::elide(ui, &label, font, t::TEXT_1, w - 40.0);
    ui.painter().galley(
        pos2(rect.min.x + 12.0, rect.center().y - g.size().y / 2.0),
        g,
        t::TEXT_1,
    );
    let x = pos2(rect.max.x - 14.0, rect.center().y);
    let xc = if resp.hovered() { t::TEXT } else { t::TEXT_3 };
    ui.painter().line_segment(
        [x + vec2(-3.5, -3.5), x + vec2(3.5, 3.5)],
        Stroke::new(1.4, xc),
    );
    ui.painter().line_segment(
        [x + vec2(3.5, -3.5), x + vec2(-3.5, 3.5)],
        Stroke::new(1.4, xc),
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
