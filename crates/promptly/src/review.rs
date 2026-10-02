//! Review pane, modelled on a pull request's "Files changed" tab: a file
//! tree, every changed file's diff stacked in one virtualised scroll with
//! sticky file headers, "Viewed" checkboxes, collapsible files, unified or
//! split view, and line comments sent back to the session as one prompt.

use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};
use parking_lot::Mutex;
use promptly_core::git::{self, DiffLine, DiffScope, FileDiff, FileStatus, LineKind, ReviewDiff};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::theme::{self, tokens as t};
use crate::ui_kit::{self as kit, Icon};

#[derive(Clone)]
pub struct Comment {
    pub file: String,
    pub line: Option<u32>,
    pub quote: String,
    pub text: String,
}

type DiffSlot = Arc<Mutex<Option<Result<ReviewDiff, String>>>>;

/// Files longer than this start folded, like GitHub's "Load diff".
const LARGE_DIFF: usize = 400;
/// The pane also refreshes on a timer, for edits made outside Claude.
const AUTO_REFRESH: Duration = Duration::from_secs(8);

const HEADER_H: f32 = 44.0;
const LINE_H: f32 = 20.0;
const NOTE_H: f32 = 44.0;
const COMMENT_H: f32 = 40.0;
const GAP_H: f32 = 14.0;
const TREE_W: f32 = 240.0;

#[derive(Default)]
pub struct ReviewState {
    pub dir: Option<PathBuf>,
    loaded: Option<(u64, DiffScope)>,
    loaded_at: Option<Instant>,
    result: DiffSlot,
    loading: bool,
    diff: Option<ReviewDiff>,
    error: Option<String>,
    pub scope: DiffScope,
    /// The session's folder is inside a git repository (checked on change).
    is_repo: bool,
    /// Files the session's agent edited, as they were before (set by the app).
    pub baselines: std::sync::Arc<promptly_core::session_changes::Baselines>,
    pub split: bool,
    viewed: HashSet<String>,
    collapsed: HashSet<String>,
    expanded_large: HashSet<String>,
    scroll_to: Option<usize>,
    selected: Option<usize>,
    pub comments: Vec<Comment>,
    draft: Option<Draft>,
}

struct Draft {
    file: String,
    line: Option<u32>,
    quote: String,
    text: String,
}

pub enum ReviewAction {
    Close,
    ToggleFull,
    OpenInEditor(PathBuf, Option<u32>),
    SendFeedback(String),
}

/// One virtualised row of the diff column.
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Line(usize, usize),
    Split(usize, Option<usize>, Option<usize>),
    Comment(usize, usize),
    Note(usize),
    Gap,
}

impl Row {
    fn height(self) -> f32 {
        match self {
            Row::Header(_) => HEADER_H,
            Row::Line(..) | Row::Split(..) => LINE_H,
            Row::Comment(..) => COMMENT_H,
            Row::Note(_) => NOTE_H,
            Row::Gap => GAP_H,
        }
    }
}

impl ReviewState {
    /// Reload when the session's folder changes, Claude reports file edits,
    /// the scope changes, or every few seconds.
    pub fn sync(&mut self, dir: &Path, seq: u64, ctx: &egui::Context) {
        if self.dir.as_deref() != Some(dir) {
            self.dir = Some(dir.to_path_buf());
            self.diff = None;
            self.error = None;
            self.comments.clear();
            self.viewed.clear();
            self.collapsed.clear();
            self.selected = None;
            self.loaded = None;
            // Outside a repository only the session's own edits can be shown.
            self.is_repo = git::repo_root(dir).is_some();
            if !self.is_repo {
                self.scope = DiffScope::Session;
            } else if self.scope == DiffScope::Session && self.baselines.is_empty() {
                self.scope = DiffScope::Branch;
            }
        }
        let stale = self.loaded_at.is_none_or(|t| t.elapsed() > AUTO_REFRESH);
        if (self.loaded != Some((seq, self.scope)) || stale) && !self.loading {
            self.loaded = Some((seq, self.scope));
            self.reload(ctx);
        }
        if let Some(r) = self.result.lock().take() {
            self.loading = false;
            self.loaded_at = Some(Instant::now());
            match r {
                Ok(d) => {
                    // Keep "viewed" only for files still in the diff.
                    let paths: HashSet<_> = d.files.iter().map(|f| f.path.clone()).collect();
                    self.viewed.retain(|p| paths.contains(p));
                    self.diff = Some(d);
                    self.error = None;
                }
                Err(e) => {
                    self.diff = None;
                    self.error = Some(e);
                }
            }
        }
        ctx.request_repaint_after(AUTO_REFRESH);
    }

    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else { return };
        self.loading = true;
        let slot = self.result.clone();
        let ctx = ctx.clone();
        let scope = self.scope;
        let baselines = self.baselines.clone();
        std::thread::spawn(move || {
            let r = if scope == DiffScope::Session {
                promptly_core::session_changes::diff(&dir, &baselines)
            } else {
                git::review_diff(&dir, scope)
            };
            *slot.lock() = Some(r);
            ctx.request_repaint();
        });
    }

    fn is_folded(&self, f: &FileDiff) -> bool {
        self.collapsed.contains(&f.path) || self.viewed.contains(&f.path)
    }

    fn rows(&self, d: &ReviewDiff) -> Vec<Row> {
        let mut rows = Vec::new();
        for (fi, f) in d.files.iter().enumerate() {
            rows.push(Row::Header(fi));
            if self.is_folded(f) {
                rows.push(Row::Gap);
                continue;
            }
            let folded_large = f.lines.len() > LARGE_DIFF && !self.expanded_large.contains(&f.path);
            let only_meta = f.lines.iter().all(|l| l.kind == LineKind::Meta);
            if f.binary || folded_large || only_meta {
                rows.push(Row::Note(fi));
            } else if self.split {
                for (l, r) in split_indices(&f.lines) {
                    rows.push(Row::Split(fi, l, r));
                }
            } else {
                for li in 0..f.lines.len() {
                    rows.push(Row::Line(fi, li));
                    let l = &f.lines[li];
                    let no = l.new_no.or(l.old_no);
                    for (ci, c) in self.comments.iter().enumerate() {
                        if c.file == f.path
                            && c.line == no
                            && no.is_some()
                            && l.kind != LineKind::Hunk
                        {
                            rows.push(Row::Comment(fi, ci));
                        }
                    }
                }
            }
            rows.push(Row::Gap);
        }
        rows
    }

    pub fn show(&mut self, ui: &mut egui::Ui, full: bool) -> Option<ReviewAction> {
        let mut action = None;

        // ------------------------------------------------ header bar
        let (hrect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        ui.painter().line_segment(
            [hrect.left_bottom(), hrect.right_bottom()],
            Stroke::new(1.0, t::BORDER),
        );
        let mut h = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(hrect.shrink2(vec2(12.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        h.label(RichText::new("Changes").size(14.0).color(t::TEXT));
        let mut scope = self.scope;
        if self.is_repo {
            seg(
                &mut h,
                ("seg-scope",),
                &mut scope,
                &[
                    (DiffScope::Branch, "Branch"),
                    (DiffScope::Uncommitted, "Uncommitted"),
                    (DiffScope::Session, "This session"),
                ],
            );
        } else {
            seg(
                &mut h,
                ("seg-scope",),
                &mut scope,
                &[(DiffScope::Session, "This session")],
            );
        }
        if scope != self.scope {
            self.scope = scope;
            self.loaded = None;
        }
        h.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if kit::icon_button(ui, Icon::Close, "Close review", false).clicked() {
                action = Some(ReviewAction::Close);
            }
            let tip = if full {
                "Back to the terminal"
            } else {
                "Expand review"
            };
            if kit::icon_button(ui, Icon::PanelRight, tip, full).clicked() {
                action = Some(ReviewAction::ToggleFull);
            }
            if kit::icon_button(ui, Icon::Refresh, "Refresh", self.loading).clicked() {
                self.loaded = None;
            }
            ui.add_space(6.0);
            seg2(
                ui,
                ("seg-split",),
                &mut self.split,
                (true, "Split"),
                (false, "Unified"),
            );
        });

        // ------------------------------------------------ states
        if let Some(e) = &self.error {
            let dir = self.dir.as_ref().map(|d| short(d)).unwrap_or_default();
            empty_state(
                ui,
                if e.contains("not inside a git repository") {
                    "Not a git repository"
                } else {
                    "Couldn't read changes"
                },
                &if e.contains("not inside a git repository") {
                    format!(
                        "This session is in {dir}. Open a session inside a repository, or cd into one; the review follows the session's folder."
                    )
                } else {
                    e.clone()
                },
            );
            return action;
        }
        let Some(d) = self.diff.clone() else {
            empty_state(ui, "Loading changes…", "");
            return action;
        };

        // ------------------------------------------------ summary
        let (add, rem) = d.totals();
        let viewed_n = d
            .files
            .iter()
            .filter(|f| self.viewed.contains(&f.path))
            .count();
        let summary = ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 46.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_space(12.0);
                let n = d.files.len();
                ui.label(
                    RichText::new(format!("{n} file{} changed", if n == 1 { "" } else { "s" }))
                        .size(13.0)
                        .color(t::TEXT),
                );
                ui.label(
                    RichText::new(format!("+{add}"))
                        .size(13.0)
                        .color(theme::GREEN),
                );
                ui.label(
                    RichText::new(format!("−{rem}"))
                        .size(13.0)
                        .color(theme::RED),
                );
                let ctx_line = match (&d.base, d.scope) {
                    (Some(b), DiffScope::Branch) => format!(
                        "{} vs {b}{}",
                        d.branch.clone().unwrap_or_default(),
                        if d.ahead > 0 {
                            format!(
                                ", {} commit{} ahead",
                                d.ahead,
                                if d.ahead == 1 { "" } else { "s" }
                            )
                        } else {
                            String::new()
                        }
                    ),
                    (_, DiffScope::Session) => {
                        "Edited by this session's agent, live as it works".to_string()
                    }
                    _ if self.scope == DiffScope::Branch => format!(
                        "{}: no base branch, showing uncommitted changes",
                        d.branch.clone().unwrap_or_default()
                    ),
                    _ => "Uncommitted changes".to_string(),
                };
                ui.label(RichText::new(ctx_line).size(12.0).color(t::TEXT_3));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    if n > 0 {
                        let frac = viewed_n as f32 / n as f32;
                        kit::meter(
                            ui,
                            frac,
                            if viewed_n == n {
                                theme::GREEN
                            } else {
                                t::TEXT_2
                            },
                            60.0,
                        );
                        ui.label(
                            RichText::new(format!("{viewed_n} / {n} viewed"))
                                .size(12.0)
                                .color(t::TEXT_2),
                        );
                    }
                });
            },
        );
        ui.painter().line_segment(
            [
                summary.response.rect.left_bottom(),
                summary.response.rect.right_bottom(),
            ],
            Stroke::new(1.0, t::BORDER),
        );

        if d.files.is_empty() {
            let (title, body) = match d.scope {
                DiffScope::Session => (
                    "No edits yet",
                    "Files Claude edits in this session appear here as it works, in any folder. Changes made by shell commands show under Uncommitted in a git repository.",
                ),
                DiffScope::Branch => (
                    "No changes",
                    "This branch matches its base and has nothing uncommitted.",
                ),
                DiffScope::Uncommitted => (
                    "No changes",
                    "Nothing uncommitted. Switch to Branch to see committed work.",
                ),
            };
            empty_state(ui, title, body);
            return action;
        }

        // Pending comments and the comment editor sit above the diff.
        self.comment_bar(ui, &mut action);

        // ------------------------------------------------ tree + diff
        let body = ui.available_rect_before_wrap();
        let show_tree = body.width() >= 640.0;
        let (tree_rect, diff_rect) = if show_tree {
            let tr = Rect::from_min_size(body.min, vec2(TREE_W, body.height()));
            let dr = Rect::from_min_max(pos2(body.min.x + TREE_W + 1.0, body.min.y), body.max);
            (Some(tr), dr)
        } else {
            (None, body)
        };
        if let Some(tr) = tree_rect {
            ui.painter().line_segment(
                [tr.right_top(), tr.right_bottom()],
                Stroke::new(1.0, t::BORDER),
            );
            let mut tu = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(tr.shrink2(vec2(8.0, 8.0)))
                    .id_salt("review-tree"),
            );
            self.tree(&mut tu, &d, &mut action);
        }
        let mut du = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(diff_rect)
                .id_salt("review-diff"),
        );
        if !show_tree {
            self.file_jump(&mut du, &d);
        }
        self.diff_column(&mut du, &d, &mut action);
        ui.advance_cursor_after_rect(body);
        action
    }

    // ---------------------------------------------------------------- tree

    fn tree(&mut self, ui: &mut egui::Ui, d: &ReviewDiff, action: &mut Option<ReviewAction>) {
        let _ = action;
        egui::ScrollArea::vertical()
            .id_salt("tree-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Group by directory so each folder appears once.
                let mut order: Vec<usize> = (0..d.files.len()).collect();
                order.sort_by_cached_key(|&i| split_path(&d.files[i].path));
                let mut last_dir: Option<String> = None;
                for fi in order {
                    let f = &d.files[fi];
                    let (dir, name) = split_path(&f.path);
                    if last_dir.as_deref() != Some(dir.as_str()) {
                        if !dir.is_empty() {
                            let (r, _) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 26.0),
                                Sense::hover(),
                            );
                            kit::paint_icon(
                                ui,
                                Rect::from_center_size(
                                    pos2(r.min.x + 10.0, r.center().y + 2.0),
                                    vec2(12.0, 12.0),
                                ),
                                kit::Icon::ChevronDown,
                                t::TEXT_3,
                            );
                            let g = kit::elide(
                                ui,
                                dir.trim_end_matches('/'),
                                FontId::proportional(11.5),
                                t::TEXT_2,
                                r.width() - 26.0,
                            );
                            ui.painter().galley(
                                pos2(r.min.x + 20.0, r.center().y + 2.0 - g.size().y / 2.0),
                                g,
                                t::TEXT_2,
                            );
                        }
                        last_dir = Some(dir.clone());
                    }
                    let indent = if dir.is_empty() { 6.0 } else { 20.0 };
                    let (r, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                    let selected = self.selected == Some(fi);
                    let h = kit::hover_t(ui, &resp);
                    let bg = if selected {
                        t::ACTIVE
                    } else {
                        kit::mix(Color32::TRANSPARENT, t::HOVER, h)
                    };
                    ui.painter().rect_filled(r, CornerRadius::same(6), bg);
                    let viewed = self.viewed.contains(&f.path);
                    let (letter, col) = status_style(f.status);
                    let p = ui.painter();
                    p.text(
                        pos2(r.min.x + indent, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        letter,
                        FontId::monospace(11.0),
                        col,
                    );
                    let name_col = if viewed {
                        t::TEXT_3
                    } else if selected {
                        t::TEXT
                    } else {
                        t::TEXT_1
                    };
                    let g = kit::elide(
                        ui,
                        &name,
                        FontId::proportional(12.5),
                        name_col,
                        r.width() - indent - 70.0,
                    );
                    ui.painter().galley(
                        pos2(r.min.x + indent + 16.0, r.center().y - g.size().y / 2.0),
                        g,
                        name_col,
                    );
                    if viewed {
                        kit::paint_icon(
                            ui,
                            Rect::from_center_size(
                                pos2(r.max.x - 12.0, r.center().y),
                                vec2(13.0, 13.0),
                            ),
                            kit::Icon::Check,
                            t::TEXT_3,
                        );
                    } else {
                        diff_stat(ui, pos2(r.max.x - 6.0, r.center().y), f.added, f.removed);
                    }
                    if resp
                        .on_hover_text(&f.path)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.selected = Some(fi);
                        self.scroll_to = Some(fi);
                        self.collapsed.remove(&f.path);
                    }
                }
            });
    }

    /// Narrow layout: a file picker instead of the tree.
    fn file_jump(&mut self, ui: &mut egui::Ui, d: &ReviewDiff) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let label = self
                .selected
                .and_then(|i| d.files.get(i))
                .map(|f| f.path.clone())
                .unwrap_or_else(|| "Jump to file…".into());
            egui::ComboBox::from_id_salt("jump")
                .selected_text(label)
                .width(ui.available_width() - 20.0)
                .show_ui(ui, |ui| {
                    for (fi, f) in d.files.iter().enumerate() {
                        if ui
                            .selectable_label(
                                self.selected == Some(fi),
                                format!("{}  {}", f.status.letter(), f.path),
                            )
                            .clicked()
                        {
                            self.selected = Some(fi);
                            self.scroll_to = Some(fi);
                        }
                    }
                });
        });
        ui.add_space(4.0);
    }

    // ---------------------------------------------------------------- diff

    fn diff_column(
        &mut self,
        ui: &mut egui::Ui,
        d: &ReviewDiff,
        action: &mut Option<ReviewAction>,
    ) {
        let rows = self.rows(d);
        // Offsets of every row, for virtualisation and jump-to-file.
        let mut offsets = Vec::with_capacity(rows.len() + 1);
        let mut y = 8.0;
        for r in &rows {
            offsets.push(y);
            y += r.height();
        }
        let total = y + 8.0;
        let header_y: Vec<(usize, f32, f32)> = {
            // (file, header top, file bottom)
            let mut v: Vec<(usize, f32, f32)> = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                if let Row::Header(fi) = r {
                    if let Some(last) = v.last_mut() {
                        last.2 = offsets[i];
                    }
                    v.push((*fi, offsets[i], total));
                }
            }
            v
        };

        let mut area = egui::ScrollArea::vertical()
            .id_salt("diff-scroll")
            .auto_shrink([false, false]);
        if let Some(fi) = self.scroll_to.take()
            && let Some((_, top, _)) = header_y.iter().find(|h| h.0 == fi)
        {
            area = area.vertical_scroll_offset((top - 8.0).max(0.0));
        }

        let mut clicks: Vec<RowClick> = Vec::new();
        area.show_viewport(ui, |ui, viewport| {
            ui.set_height(total);
            let origin = ui.min_rect().min;
            let width = ui.available_width();
            let first = offsets
                .partition_point(|&o| o < viewport.min.y - 60.0)
                .saturating_sub(1);
            for (i, row) in rows.iter().enumerate().skip(first) {
                let top = offsets[i];
                if top > viewport.max.y {
                    break;
                }
                let rect = Rect::from_min_size(origin + vec2(0.0, top), vec2(width, row.height()));
                if let Some(c) = self.paint_row(ui, rect, *row, d, i) {
                    clicks.push(c);
                }
            }
            // Sticky header for the file under the top edge.
            if let Some(&(fi, top, bottom)) = header_y
                .iter()
                .find(|h| h.1 < viewport.min.y && h.2 > viewport.min.y + HEADER_H)
            {
                let _ = top;
                let y = origin.y + viewport.min.y.min(bottom - HEADER_H);
                let rect = Rect::from_min_size(pos2(origin.x, y), vec2(width, HEADER_H));
                if let Some(c) = self.paint_header(ui, rect, d, fi, true) {
                    clicks.push(c);
                }
            }
        });

        for c in clicks {
            match c {
                RowClick::ToggleFold(fi) => {
                    let p = d.files[fi].path.clone();
                    if self.viewed.contains(&p) {
                        self.viewed.remove(&p);
                    } else if !self.collapsed.remove(&p) {
                        self.collapsed.insert(p);
                    }
                    self.selected = Some(fi);
                }
                RowClick::ToggleViewed(fi) => {
                    let p = d.files[fi].path.clone();
                    if !self.viewed.remove(&p) {
                        self.viewed.insert(p);
                        // Like GitHub: marking viewed folds the file and keeps your place.
                        self.scroll_to = Some(fi);
                    }
                }
                RowClick::Open(fi, line) => {
                    *action = Some(ReviewAction::OpenInEditor(
                        d.root.join(&d.files[fi].path),
                        line,
                    ));
                }
                RowClick::ShowLarge(fi) => {
                    self.expanded_large.insert(d.files[fi].path.clone());
                }
                RowClick::Comment(fi, li) => {
                    let f = &d.files[fi];
                    let l = &f.lines[li];
                    self.draft = Some(Draft {
                        file: f.path.clone(),
                        line: l.new_no.or(l.old_no),
                        quote: l.text.clone(),
                        text: String::new(),
                    });
                }
                RowClick::DeleteComment(ci) => {
                    if ci < self.comments.len() {
                        self.comments.remove(ci);
                    }
                }
            }
        }
    }

    fn paint_row(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        row: Row,
        d: &ReviewDiff,
        idx: usize,
    ) -> Option<RowClick> {
        match row {
            Row::Header(fi) => self.paint_header(ui, rect, d, fi, false),
            Row::Gap => None,
            Row::Note(fi) => {
                let f = &d.files[fi];
                let r = rect.shrink2(vec2(12.0, 0.0));
                ui.painter().rect_filled(
                    r,
                    CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 8,
                        se: 8,
                    },
                    t::BG_MAIN,
                );
                let (text, link) = if f.binary {
                    ("Binary file not shown.".to_string(), None)
                } else if f.lines.len() > LARGE_DIFF {
                    (
                        format!("Large diff, {} lines.", f.lines.len()),
                        Some("Load diff"),
                    )
                } else {
                    (
                        f.lines
                            .iter()
                            .map(|l| l.text.clone())
                            .collect::<Vec<_>>()
                            .join(" · "),
                        None,
                    )
                };
                ui.painter().text(
                    pos2(r.min.x + 16.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    &text,
                    FontId::proportional(12.5),
                    t::TEXT_2,
                );
                if let Some(l) = link {
                    let lr = Rect::from_min_size(
                        pos2(r.min.x + 30.0 + text.len() as f32 * 6.6, r.min.y + 10.0),
                        vec2(80.0, 24.0),
                    );
                    let resp = ui.interact(lr, ui.id().with(("load", fi)), Sense::click());
                    ui.painter().text(
                        lr.left_center(),
                        egui::Align2::LEFT_CENTER,
                        l,
                        FontId::proportional(12.5),
                        theme::BLUE,
                    );
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        return Some(RowClick::ShowLarge(fi));
                    }
                }
                None
            }
            Row::Line(fi, li) => {
                let f = &d.files[fi];
                let l = &f.lines[li];
                let r = rect.shrink2(vec2(12.0, 0.0));
                let commented = self.comments.iter().any(|c| {
                    c.file == f.path && c.line == l.new_no.or(l.old_no) && c.line.is_some()
                });
                let resp = ui.interact(r, ui.id().with(("ln", idx)), Sense::click());
                paint_line(ui, r, Some(l), true, commented, resp.hovered());
                if l.kind != LineKind::Hunk && l.kind != LineKind::Meta {
                    let resp = resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Click to comment on this line");
                    if resp.clicked() {
                        return Some(RowClick::Comment(fi, li));
                    }
                }
                None
            }
            Row::Split(fi, l, r) => {
                let f = &d.files[fi];
                let inner = rect.shrink2(vec2(12.0, 0.0));
                let half = inner.width() / 2.0;
                let lr = Rect::from_min_size(inner.min, vec2(half, inner.height()));
                let rr =
                    Rect::from_min_size(inner.min + vec2(half, 0.0), vec2(half, inner.height()));
                let left = l.map(|i| &f.lines[i]).filter(|x| x.kind != LineKind::Added);
                let right = r
                    .map(|i| &f.lines[i])
                    .filter(|x| x.kind != LineKind::Removed);
                paint_line(ui, lr, left, l.is_some(), false, false);
                paint_line(ui, rr, right, r.is_some(), false, false);
                ui.painter().line_segment(
                    [rr.left_top(), rr.left_bottom()],
                    Stroke::new(1.0, t::BORDER),
                );
                None
            }
            Row::Comment(_fi, ci) => {
                let c = &self.comments[ci];
                let r = rect.shrink2(vec2(12.0, 0.0)).shrink2(vec2(40.0, 4.0));
                ui.painter().rect(
                    r,
                    CornerRadius::same(8),
                    t::BG_ELEVATED,
                    Stroke::new(1.0, t::ACCENT.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
                let g = kit::elide(
                    ui,
                    &c.text,
                    FontId::proportional(12.5),
                    t::TEXT,
                    r.width() - 90.0,
                );
                ui.painter().galley(
                    pos2(r.min.x + 12.0, r.center().y - g.size().y / 2.0),
                    g,
                    t::TEXT,
                );
                let del = Rect::from_min_size(
                    pos2(r.max.x - 64.0, r.min.y + 6.0),
                    vec2(56.0, r.height() - 12.0),
                );
                let resp = ui.interact(del, ui.id().with(("del", ci)), Sense::click());
                ui.painter().text(
                    del.center(),
                    egui::Align2::CENTER_CENTER,
                    "Delete",
                    FontId::proportional(11.5),
                    if resp.hovered() {
                        theme::RED
                    } else {
                        t::TEXT_3
                    },
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    return Some(RowClick::DeleteComment(ci));
                }
                None
            }
        }
    }

    /// File header: fold chevron, status, path, +/− bar, Viewed, Open.
    fn paint_header(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        d: &ReviewDiff,
        fi: usize,
        sticky: bool,
    ) -> Option<RowClick> {
        let f = &d.files[fi];
        let r = rect.shrink2(vec2(12.0, 0.0)).with_max_y(rect.max.y - 4.0);
        let folded = self.is_folded(f);
        let viewed = self.viewed.contains(&f.path);
        let radius = if folded {
            CornerRadius::same(8)
        } else {
            CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            }
        };
        let p = ui.painter();
        if sticky {
            p.rect_filled(rect.with_max_y(r.max.y), 0.0, t::BG_SIDEBAR);
        }
        p.rect(
            r,
            radius,
            t::BG_ELEVATED,
            Stroke::new(1.0, t::BORDER),
            egui::StrokeKind::Inside,
        );
        let id = ui.id().with(("hdr", fi, sticky));
        let mut click = None;

        // Chevron + path area toggles folding.
        let fold_r = Rect::from_min_max(r.min, pos2(r.max.x - 140.0, r.max.y));
        let fold = ui.interact(fold_r, id.with("fold"), Sense::click());
        let cx = r.min.x + 16.0;
        let cy = r.center().y;
        kit::paint_icon(
            ui,
            Rect::from_center_size(pos2(cx, cy), vec2(14.0, 14.0)),
            if folded {
                kit::Icon::ChevronRight
            } else {
                kit::Icon::ChevronDown
            },
            t::TEXT_2,
        );
        let (letter, col) = status_style(f.status);
        let badge = Rect::from_center_size(pos2(r.min.x + 40.0, cy), vec2(18.0, 18.0));
        ui.painter()
            .rect_filled(badge, CornerRadius::same(5), col.gamma_multiply(0.14));
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            FontId::monospace(11.0),
            col,
        );

        // Path: directory muted, file name bright; renames show old → new.
        let (dir, name) = split_path(&f.path);
        let mut job = egui::text::LayoutJob::default();
        if let Some(old) = &f.old_path {
            job.append(
                &format!("{old} → "),
                0.0,
                egui::TextFormat::simple(FontId::monospace(12.0), t::TEXT_3),
            );
        }
        job.append(
            &dir,
            0.0,
            egui::TextFormat::simple(FontId::monospace(12.0), t::TEXT_3),
        );
        job.append(
            &name,
            0.0,
            egui::TextFormat::simple(
                FontId::monospace(12.0),
                if viewed { t::TEXT_2 } else { t::TEXT },
            ),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: fold_r.width() - 150.0,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let g = ui.fonts_mut(|fo| fo.layout_job(job));
        let gw = g.size().x;
        ui.painter()
            .galley(pos2(r.min.x + 58.0, cy - g.size().y / 2.0), g, t::TEXT);

        // +N −M and the five-block bar, like GitHub.
        let stat_x = r.min.x + 66.0 + gw;
        let stat_w = diff_stat_width(ui, f.added, f.removed);
        diff_stat(ui, pos2(stat_x + stat_w, cy), f.added, f.removed);
        let blocks_x = stat_x + stat_w + 8.0;
        let total = (f.added + f.removed).max(1) as f32;
        let green = ((f.added as f32 / total) * 5.0).round() as usize;
        let red = if f.removed > 0 { (5 - green).max(1) } else { 0 };
        for b in 0..5 {
            let c = if b < green {
                theme::GREEN.gamma_multiply(0.85)
            } else if b < green + red {
                theme::RED.gamma_multiply(0.85)
            } else {
                t::BORDER_STRONG
            };
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(blocks_x + b as f32 * 8.0, cy - 3.5), vec2(7.0, 7.0)),
                CornerRadius::same(2),
                c,
            );
        }
        if fold
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            click = Some(RowClick::ToggleFold(fi));
        }

        // Viewed checkbox: borderless until hovered.
        let vr = Rect::from_min_size(pos2(r.max.x - 132.0, cy - 13.0), vec2(84.0, 26.0));
        let vresp = ui.interact(vr, id.with("viewed"), Sense::click());
        let vh = kit::hover_t(ui, &vresp);
        ui.painter().rect_filled(
            vr,
            CornerRadius::same(6),
            kit::mix(Color32::TRANSPARENT, t::HOVER, vh),
        );
        let box_r = Rect::from_center_size(pos2(vr.min.x + 15.0, cy), vec2(14.0, 14.0));
        ui.painter().rect(
            box_r,
            CornerRadius::same(4),
            if viewed {
                t::ACCENT
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(
                1.2,
                if viewed {
                    t::ACCENT
                } else {
                    kit::mix(t::TEXT_3, t::TEXT_2, vh)
                },
            ),
            egui::StrokeKind::Inside,
        );
        if viewed {
            kit::paint_icon(ui, box_r.shrink(2.0), kit::Icon::Check, Color32::WHITE);
        }
        ui.painter().text(
            pos2(vr.min.x + 29.0, cy),
            egui::Align2::LEFT_CENTER,
            "Viewed",
            FontId::proportional(12.0),
            if viewed { t::TEXT_1 } else { t::TEXT_2 },
        );
        if vresp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Mark as viewed and fold")
            .clicked()
        {
            click = Some(RowClick::ToggleViewed(fi));
        }

        // Open in editor.
        let or = Rect::from_min_size(pos2(r.max.x - 40.0, cy - 13.0), vec2(28.0, 26.0));
        let oresp = ui.interact(or, id.with("open"), Sense::click());
        let oh = kit::hover_t(ui, &oresp);
        ui.painter().rect_filled(
            or,
            CornerRadius::same(6),
            kit::mix(Color32::TRANSPARENT, t::HOVER, oh),
        );
        kit::paint_icon(
            ui,
            or.shrink2(vec2(7.0, 6.5)),
            kit::Icon::External,
            kit::mix(t::TEXT_2, t::TEXT, oh),
        );
        let oresp = oresp.on_hover_text("Open in editor");
        if f.status != FileStatus::Deleted
            && oresp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            let line = f.lines.iter().find_map(|l| {
                if l.kind == LineKind::Added {
                    l.new_no
                } else {
                    None
                }
            });
            click = Some(RowClick::Open(fi, line));
        }
        click
    }

    // ---------------------------------------------------------------- comments

    fn comment_bar(&mut self, ui: &mut egui::Ui, action: &mut Option<ReviewAction>) {
        if !self.comments.is_empty() && self.draft.is_none() {
            egui::Frame::new()
                .fill(t::ACCENT.gamma_multiply(0.10))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .outer_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let n = self.comments.len();
                        ui.label(
                            RichText::new(format!(
                                "{n} pending comment{}",
                                if n == 1 { "" } else { "s" }
                            ))
                            .color(t::TEXT_1),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::primary_button(
                                ui,
                                Some(Icon::Send),
                                "Send review to session",
                                None,
                                false,
                            )
                            .clicked()
                            {
                                *action = Some(ReviewAction::SendFeedback(feedback_prompt(
                                    &self.comments,
                                )));
                                self.comments.clear();
                            }
                            if ui.small_button("Discard").clicked() {
                                self.comments.clear();
                            }
                        });
                    });
                });
        }
        let mut done = None;
        if let Some(dr) = self.draft.as_mut() {
            egui::Frame::new()
                .fill(t::BG_ELEVATED)
                .stroke(Stroke::new(1.0, t::BORDER_STRONG))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .outer_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Comment on {}{}",
                            dr.file,
                            dr.line.map(|l| format!(":{l}")).unwrap_or_default()
                        ))
                        .size(12.0)
                        .color(t::TEXT_2),
                    );
                    ui.label(
                        RichText::new(dr.quote.trim())
                            .monospace()
                            .size(11.5)
                            .color(t::TEXT_3),
                    );
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut dr.text)
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
                            if kit::primary_button(ui, None, "Add comment", Some("⌘↵"), false)
                                .clicked()
                                || enter
                            {
                                done = Some(true);
                            }
                        });
                    });
                });
        }
        match done {
            Some(true) => {
                if let Some(dr) = self.draft.take()
                    && !dr.text.trim().is_empty()
                {
                    self.comments.push(Comment {
                        file: dr.file,
                        line: dr.line,
                        quote: dr.quote,
                        text: dr.text.trim().to_string(),
                    });
                }
            }
            Some(false) => self.draft = None,
            None => {}
        }
    }
}

enum RowClick {
    ToggleFold(usize),
    ToggleViewed(usize),
    Open(usize, Option<u32>),
    ShowLarge(usize),
    Comment(usize, usize),
    DeleteComment(usize),
}

/// Index pairs for split view: removed/added runs side by side.
fn split_indices(lines: &[DiffLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Removed | LineKind::Added => {
                let s = i;
                while i < lines.len() && lines[i].kind == LineKind::Removed {
                    i += 1;
                }
                let rem: Vec<usize> = (s..i).collect();
                let a = i;
                while i < lines.len() && lines[i].kind == LineKind::Added {
                    i += 1;
                }
                let add: Vec<usize> = (a..i).collect();
                for k in 0..rem.len().max(add.len()) {
                    rows.push((rem.get(k).copied(), add.get(k).copied()));
                }
            }
            _ => {
                rows.push((Some(i), Some(i)));
                i += 1;
            }
        }
    }
    rows
}

/// One diff line with gutter, sign and colouring. `None` paints an empty cell.
fn paint_line(
    ui: &egui::Ui,
    rect: Rect,
    l: Option<&DiffLine>,
    present: bool,
    commented: bool,
    hovered: bool,
) {
    let p = ui.painter_at(rect);
    let Some(l) = l else {
        let fill = if present { t::BG_MAIN } else { t::BG_SIDEBAR };
        p.rect_filled(rect, 0.0, fill);
        return;
    };
    let (bg, fg, sign) = match l.kind {
        LineKind::Added => (
            Color32::from_rgba_unmultiplied(0x3f, 0xb9, 0x50, 16),
            t::TEXT_1,
            "+",
        ),
        LineKind::Removed => (
            Color32::from_rgba_unmultiplied(0xe5, 0x53, 0x4b, 16),
            t::TEXT_1,
            "−",
        ),
        LineKind::Hunk => (t::BG_SIDEBAR, t::TEXT_3, ""),
        LineKind::Meta => (t::BG_MAIN, t::TEXT_3, ""),
        LineKind::Context => (t::BG_MAIN, t::TEXT_2, " "),
    };
    p.rect_filled(
        rect,
        0.0,
        if hovered && l.kind != LineKind::Hunk {
            t::HOVER
        } else {
            bg
        },
    );
    let mono = FontId::monospace(11.5);
    let gutter = 92.0;
    if l.kind != LineKind::Hunk {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(gutter - 8.0, rect.height())),
            0.0,
            Color32::from_black_alpha(28),
        );
        let edge = match l.kind {
            LineKind::Added => Some(theme::GREEN),
            LineKind::Removed => Some(theme::RED),
            _ => None,
        };
        if let Some(c) = edge {
            p.rect_filled(
                Rect::from_min_size(
                    pos2(rect.min.x + gutter - 10.0, rect.min.y),
                    vec2(2.0, rect.height()),
                ),
                0.0,
                c.gamma_multiply(0.8),
            );
        }
        if let Some(n) = l.old_no {
            p.text(
                pos2(rect.min.x + 38.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                mono.clone(),
                t::TEXT_3,
            );
        }
        if let Some(n) = l.new_no {
            p.text(
                pos2(rect.min.x + 76.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                mono.clone(),
                t::TEXT_3,
            );
        }
    }
    if commented {
        p.circle_filled(
            pos2(rect.min.x + 6.0, rect.center().y),
            3.0,
            t::ACCENT_HOVER,
        );
    }
    let sign_col = match l.kind {
        LineKind::Added => theme::GREEN,
        LineKind::Removed => theme::RED,
        _ => t::TEXT_3,
    };
    let text = if l.kind == LineKind::Hunk {
        l.text.clone()
    } else {
        p.text(
            pos2(rect.min.x + gutter, rect.center().y),
            egui::Align2::LEFT_CENTER,
            sign,
            mono.clone(),
            sign_col,
        );
        format!("  {}", l.text.replace('\t', "    "))
    };
    let x = if l.kind == LineKind::Hunk {
        rect.min.x + 12.0
    } else {
        rect.min.x + gutter
    };
    p.text(
        pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        mono,
        fg,
    );
}

fn diff_stat_text(added: u32, removed: u32) -> (String, String) {
    (format!("+{added}"), format!("−{removed}"))
}

fn diff_stat_width(ui: &egui::Ui, added: u32, removed: u32) -> f32 {
    let (a, r) = diff_stat_text(added, removed);
    let f = FontId::proportional(11.5);
    ui.fonts_mut(|fo| {
        fo.layout_no_wrap(a, f.clone(), t::TEXT).size().x
            + 6.0
            + fo.layout_no_wrap(r, f, t::TEXT).size().x
    })
}

/// "+12 −3" in green and red, right-aligned at `right`.
fn diff_stat(ui: &egui::Ui, right: egui::Pos2, added: u32, removed: u32) {
    let (a, r) = diff_stat_text(added, removed);
    let f = FontId::proportional(11.5);
    let rr = ui.painter().text(
        right,
        egui::Align2::RIGHT_CENTER,
        r,
        f.clone(),
        if removed == 0 {
            t::TEXT_3
        } else {
            theme::RED.gamma_multiply(0.9)
        },
    );
    ui.painter().text(
        pos2(rr.min.x - 6.0, right.y),
        egui::Align2::RIGHT_CENTER,
        a,
        f,
        if added == 0 {
            t::TEXT_3
        } else {
            theme::GREEN.gamma_multiply(0.9)
        },
    );
}

fn status_style(s: FileStatus) -> (&'static str, Color32) {
    match s {
        FileStatus::Added => ("A", theme::GREEN),
        FileStatus::Modified => ("M", theme::AMBER),
        FileStatus::Deleted => ("D", theme::RED),
        FileStatus::Renamed => ("R", theme::BLUE),
    }
}

/// ("src/server/", "main.rs")
fn split_path(p: &str) -> (String, String) {
    match p.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), p.to_string()),
    }
}

fn short(p: &Path) -> String {
    let home = promptly_core::paths::home();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Two-option segmented control.
fn seg2<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    a: (T, &str),
    b: (T, &str),
) {
    seg(ui, id, value, &[a, b]);
}

/// Segmented control.
fn seg<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    opts: &[(T, &str)],
) {
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = opts
        .iter()
        .map(|(_, l)| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap((*l).into(), font.clone(), t::TEXT)
                    .size()
                    .x
            }) + 24.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 26.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(7), t::BG_MAIN);
    let base = ui.id().with(id);
    let mut x = rect.min.x + 2.0;
    for ((opt, label), w) in opts.iter().zip(widths) {
        let r = Rect::from_min_size(pos2(x, rect.min.y + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, base.with(*label), Sense::click());
        let on = *value == *opt;
        if on {
            ui.painter()
                .rect_filled(r, CornerRadius::same(5), t::ACTIVE);
        }
        let col = if on || resp.hovered() {
            t::TEXT
        } else {
            t::TEXT_3
        };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            col,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *value = *opt;
        }
        x += w;
    }
}

fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(360.0);
        ui.label(RichText::new(title).size(15.0).color(t::TEXT_1));
        if !body.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(body).size(12.5).color(t::TEXT_3));
        }
    });
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

    #[test]
    fn split_pairs_runs() {
        let l = |k| DiffLine {
            kind: k,
            text: String::new(),
            old_no: None,
            new_no: None,
        };
        let lines = vec![
            l(LineKind::Context),
            l(LineKind::Removed),
            l(LineKind::Added),
            l(LineKind::Added),
        ];
        assert_eq!(
            split_indices(&lines),
            vec![(Some(0), Some(0)), (Some(1), Some(2)), (None, Some(3))]
        );
    }

    #[test]
    fn rows_fold_viewed_files() {
        let mut st = ReviewState::default();
        let f = FileDiff {
            path: "a.rs".into(),
            lines: vec![DiffLine {
                kind: LineKind::Added,
                text: "x".into(),
                old_no: None,
                new_no: Some(1),
            }],
            ..Default::default()
        };
        let d = ReviewDiff {
            files: vec![f],
            ..Default::default()
        };
        assert_eq!(st.rows(&d).len(), 3, "header, line, gap");
        st.viewed.insert("a.rs".into());
        assert_eq!(st.rows(&d).len(), 2, "viewed folds to header + gap");
    }
}
