//! Review pane: read-only diff of a session's working tree (unified or
//! split), open-in-$EDITOR, and inline comments sent back as a prompt.

use egui::{Color32, RichText};
use parking_lot::Mutex;
use promptly_core::git::{self, DiffLine, FileDiff, LineKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::theme;

#[derive(Clone)]
pub struct Comment {
    pub file: String,
    pub line: Option<u32>,
    pub quote: String,
    pub text: String,
}

type DiffSlot = Arc<Mutex<Option<Result<Vec<FileDiff>, String>>>>;

#[derive(Default)]
pub struct ReviewState {
    pub dir: Option<PathBuf>,
    loaded_seq: Option<u64>,
    result: DiffSlot,
    loading: bool,
    files: Vec<FileDiff>,
    error: Option<String>,
    selected: usize,
    pub split: bool,
    pub comments: Vec<Comment>,
    draft: Option<(String, Option<u32>, String, String)>,
}

pub enum ReviewAction {
    Close,
    OpenInEditor(PathBuf, Option<u32>),
    SendFeedback(String),
}

impl ReviewState {
    /// Reload when the session changes or reports new file changes.
    pub fn sync(&mut self, dir: &Path, seq: u64, ctx: &egui::Context) {
        if self.dir.as_deref() != Some(dir) {
            self.dir = Some(dir.to_path_buf());
            self.files.clear();
            self.comments.clear();
            self.selected = 0;
            self.loaded_seq = None;
        }
        if self.loaded_seq != Some(seq) && !self.loading {
            self.loaded_seq = Some(seq);
            self.reload(ctx);
        }
        if let Some(r) = self.result.lock().take() {
            self.loading = false;
            match r {
                Ok(f) => {
                    self.files = f;
                    self.error = None;
                    self.selected = self.selected.min(self.files.len().saturating_sub(1));
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else { return };
        self.loading = true;
        let slot = self.result.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            *slot.lock() = Some(git::working_diff(&dir));
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<ReviewAction> {
        use crate::theme::tokens as t;
        use crate::ui_kit::{self as kit, Icon};
        let mut action = None;

        // Header row, aligned with the pane header (40 px).
        let (hrect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
        ui.painter().line_segment(
            [hrect.left_bottom(), hrect.right_bottom()],
            egui::Stroke::new(1.0, t::BORDER),
        );
        let mut h = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(hrect.shrink2(egui::vec2(12.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        h.label(RichText::new("Changes").size(14.0).color(t::TEXT));
        let total: (u32, u32) = self
            .files
            .iter()
            .fold((0, 0), |a, f| (a.0 + f.added, a.1 + f.removed));
        if !self.files.is_empty() {
            kit::pill(&mut h, &format!("+{}", total.0), theme::GREEN);
            kit::pill(&mut h, &format!("−{}", total.1), theme::RED);
        }
        h.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if kit::icon_button(ui, Icon::Close, "Close review", false).clicked() {
                action = Some(ReviewAction::Close);
            }
            if kit::icon_button(ui, Icon::Refresh, "Refresh", self.loading).clicked() {
                self.loaded_seq = None;
            }
            ui.add_space(6.0);
            segmented(ui, &mut self.split);
        });

        let body = ui
            .available_rect_before_wrap()
            .shrink2(egui::vec2(8.0, 6.0));
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let ui = &mut ui;

        if let Some(e) = &self.error {
            empty_note(
                ui,
                if e.contains("not a git") {
                    "This folder is not a git repository"
                } else {
                    e
                },
            );
            return action;
        }
        if self.files.is_empty() {
            empty_note(
                ui,
                if self.loading {
                    "Loading changes…"
                } else {
                    "No changes yet"
                },
            );
            return action;
        }

        // File list
        let list_h = (self.files.len() as f32 * 28.0).min(180.0);
        egui::ScrollArea::vertical()
            .id_salt("review-files")
            .max_height(list_h)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (i, f) in self.files.iter().enumerate() {
                    if file_row(ui, f, i == self.selected).clicked() {
                        self.selected = i;
                    }
                }
            });
        ui.add_space(6.0);
        let file = self.files.get(self.selected).cloned()?;

        // File toolbar
        ui.horizontal(|ui| {
            let g = kit::elide(
                ui,
                &file.path,
                egui::FontId::monospace(12.0),
                t::TEXT_1,
                ui.available_width() - 150.0,
            );
            ui.label(g);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Open in editor").clicked()
                    && let Some(d) = &self.dir
                {
                    let root = git::repo_root(d).unwrap_or_else(|| d.clone());
                    let line = file.lines.iter().find_map(|l| l.new_no);
                    action = Some(ReviewAction::OpenInEditor(root.join(&file.path), line));
                }
            });
        });

        // Comments + send
        if !self.comments.is_empty() {
            egui::Frame::new()
                .fill(t::ACCENT.gamma_multiply(0.10))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let n = self.comments.len();
                        ui.label(
                            RichText::new(format!("{n} comment{}", if n == 1 { "" } else { "s" }))
                                .color(t::TEXT_1),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::primary_button(
                                ui,
                                Some(Icon::Send),
                                "Send to session",
                                None,
                                false,
                            )
                            .clicked()
                            {
                                action = Some(ReviewAction::SendFeedback(feedback_prompt(
                                    &self.comments,
                                )));
                                self.comments.clear();
                            }
                            if ui.small_button("Clear").clicked() {
                                self.comments.clear();
                            }
                        });
                    });
                });
            ui.add_space(4.0);
        }
        if let Some((path, line, _quote, text)) = self.draft.as_mut() {
            let mut done = None;
            egui::Frame::new()
                .fill(t::BG_ELEVATED)
                .stroke(egui::Stroke::new(1.0, t::BORDER))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Comment on {path}{}",
                            line.map(|l| format!(":{l}")).unwrap_or_default()
                        ))
                        .size(12.0)
                        .color(t::TEXT_2),
                    );
                    let r = ui.add(
                        egui::TextEdit::multiline(text)
                            .frame(egui::Frame::NONE)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("What should change here?"),
                    );
                    r.request_focus();
                    ui.horizontal(|ui| {
                        if ui.small_button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            done = Some(false);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let enter = r.has_focus()
                                && ui.input(|i| {
                                    i.key_pressed(egui::Key::Enter) && i.modifiers.command
                                });
                            if kit::primary_button(ui, None, "Add comment", None, false).clicked()
                                || enter
                            {
                                done = Some(true);
                            }
                        });
                    });
                });
            ui.add_space(4.0);
            match done {
                Some(true) if !text.trim().is_empty() => {
                    let (path, line, quote, text) = self.draft.take().unwrap();
                    self.comments.push(Comment {
                        file: path,
                        line,
                        quote,
                        text: text.trim().to_string(),
                    });
                }
                Some(_) => self.draft = None,
                None => {}
            }
        }

        // Diff body
        let row_h = 18.0;
        let commented: Vec<Option<u32>> = self
            .comments
            .iter()
            .filter(|c| c.file == file.path)
            .map(|c| c.line)
            .collect();
        let mut new_draft = None;
        egui::Frame::new()
            .fill(t::BG_MAIN)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(0, 4))
            .show(ui, |ui| {
                let rows: Vec<(Option<&DiffLine>, Option<&DiffLine>)> = if self.split {
                    git::split_rows(&file.lines)
                } else {
                    file.lines.iter().map(|l| (Some(l), None)).collect()
                };
                egui::ScrollArea::both()
                    .id_salt(("review-diff", &file.path))
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, rows.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (l, r) in &rows[range] {
                            if self.split {
                                let w = ui.available_width() / 2.0;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    diff_row(ui, side_of(*l, LineKind::Removed), w, false);
                                    diff_row(ui, side_of(*r, LineKind::Added), w, false);
                                });
                            } else if let Some(l) = l {
                                let marked = commented.contains(&l.new_no.or(l.old_no));
                                let resp = diff_row(ui, Some(l), ui.available_width(), marked);
                                if l.kind != LineKind::Hunk
                                    && l.kind != LineKind::Meta
                                    && resp
                                        .on_hover_text("Click to comment on this line")
                                        .clicked()
                                {
                                    new_draft = Some((
                                        file.path.clone(),
                                        l.new_no.or(l.old_no),
                                        l.text.clone(),
                                        String::new(),
                                    ));
                                }
                            }
                        }
                    });
            });
        if new_draft.is_some() {
            self.draft = new_draft;
        }
        action
    }
}

fn side_of(l: Option<&DiffLine>, side: LineKind) -> Option<&DiffLine> {
    l.filter(|l| {
        l.kind == side || matches!(l.kind, LineKind::Context | LineKind::Hunk | LineKind::Meta)
    })
}

/// Two-option segmented control: Unified | Split.
fn segmented(ui: &mut egui::Ui, split: &mut bool) {
    use crate::theme::tokens as t;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(124.0, 26.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(7), t::BG_MAIN);
    let half = rect.width() / 2.0;
    // Laid out right-to-left: Split is the right half.
    for (i, (label, value)) in [("Unified", false), ("Split", true)]
        .into_iter()
        .enumerate()
    {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(half * i as f32, 0.0),
            egui::vec2(half, rect.height()),
        )
        .shrink(2.0);
        let resp = ui.interact(r, ui.id().with(("seg", label)), egui::Sense::click());
        let on = *split == value;
        if on {
            ui.painter()
                .rect_filled(r, egui::CornerRadius::same(5), t::ACTIVE);
        }
        let col = if on || resp.hovered() {
            t::TEXT
        } else {
            t::TEXT_3
        };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            col,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *split = value;
        }
    }
}

fn empty_note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).color(crate::theme::tokens::TEXT_3));
    });
}

fn file_row(ui: &mut egui::Ui, f: &FileDiff, selected: bool) -> egui::Response {
    use crate::theme::tokens as t;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), egui::Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), t::ACTIVE);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), t::HOVER);
    }
    let (letter, col) = if f.untracked {
        ("A", theme::GREEN)
    } else if f.removed > 0
        && f.added == 0
        && f.lines.iter().any(|l| l.text.starts_with("deleted file"))
    {
        ("D", theme::RED)
    } else {
        ("M", theme::AMBER)
    };
    let p = ui.painter();
    p.text(
        egui::pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        letter,
        egui::FontId::monospace(11.5),
        col,
    );
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), f.path.clone()),
    };
    let counts = format!("+{} −{}", f.added, f.removed);
    let counts_w = 70.0;
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &name,
        0.0,
        egui::TextFormat::simple(
            egui::FontId::proportional(13.0),
            if selected { t::TEXT } else { t::TEXT_1 },
        ),
    );
    job.append(
        &format!("  {dir}"),
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(11.5), t::TEXT_3),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - 30.0 - counts_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = ui.fonts_mut(|fo| fo.layout_job(job));
    p.galley(
        egui::pos2(rect.min.x + 28.0, rect.center().y - g.size().y / 2.0),
        g,
        t::TEXT_1,
    );
    p.text(
        egui::pos2(rect.max.x - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        counts,
        egui::FontId::monospace(11.0),
        t::TEXT_3,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One diff line, painted full width with a line-number gutter.
fn diff_row(
    ui: &mut egui::Ui,
    l: Option<&DiffLine>,
    width: f32,
    commented: bool,
) -> egui::Response {
    use crate::theme::tokens as t;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::click());
    let Some(l) = l else {
        ui.painter()
            .rect_filled(rect, 0.0, t::BG_SIDEBAR.gamma_multiply(0.6));
        return resp;
    };
    let (bg, fg, sign) = match l.kind {
        LineKind::Added => (
            Color32::from_rgba_unmultiplied(0x3f, 0xb9, 0x50, 28),
            Color32::from_rgb(0xb7, 0xe4, 0xbf),
            "+",
        ),
        LineKind::Removed => (
            Color32::from_rgba_unmultiplied(0xe5, 0x53, 0x4b, 28),
            Color32::from_rgb(0xf0, 0xb4, 0xb0),
            "−",
        ),
        LineKind::Hunk => (
            Color32::from_rgba_unmultiplied(0x4c, 0x8d, 0xf6, 18),
            theme::BLUE,
            "",
        ),
        LineKind::Meta => (Color32::TRANSPARENT, t::TEXT_3, ""),
        LineKind::Context => (Color32::TRANSPARENT, t::TEXT_2, " "),
    };
    let p = ui.painter();
    let hover = resp.hovered()
        && matches!(
            l.kind,
            LineKind::Added | LineKind::Removed | LineKind::Context
        );
    p.rect_filled(rect, 0.0, if hover { t::HOVER } else { bg });
    let mono = egui::FontId::monospace(11.5);
    let gutter = 44.0;
    if let Some(n) = l.new_no.or(l.old_no) {
        p.text(
            egui::pos2(rect.min.x + gutter - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            mono.clone(),
            t::TEXT_3,
        );
    }
    if commented {
        p.circle_filled(
            egui::pos2(rect.min.x + 6.0, rect.center().y),
            3.0,
            t::ACCENT_HOVER,
        );
    }
    let text = if l.kind == LineKind::Hunk || l.kind == LineKind::Meta {
        l.text.clone()
    } else {
        format!("{sign} {}", l.text)
    };
    p.text(
        egui::pos2(rect.min.x + gutter, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        mono,
        fg,
    );
    resp
}

/// Turn review comments into a structured prompt for the session.
pub fn feedback_prompt(comments: &[Comment]) -> String {
    let mut s = String::from("Review feedback on your changes. Please address each comment:\n");
    for (i, c) in comments.iter().enumerate() {
        let loc = match c.line {
            Some(l) => format!("{}:{l}", c.file),
            None => c.file.clone(),
        };
        s.push_str(&format!(
            "\n{}. {loc}\n   > {}\n   {}\n",
            i + 1,
            c.quote.trim(),
            c.text
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_is_structured() {
        let p = feedback_prompt(&[Comment {
            file: "src/a.rs".into(),
            line: Some(12),
            quote: "  let x = 1;".into(),
            text: "use a const".into(),
        }]);
        assert!(p.contains("1. src/a.rs:12\n   > let x = 1;\n   use a const"));
    }
}
