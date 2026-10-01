//! Terminal pane widget: renders the VT grid with the egui painter on wgpu
//! and translates keyboard, mouse, paste and selection into PTY input.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{TermMode, point_to_viewport, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use promptly_core::sanitize::{self, PasteRisk};
use std::time::{Duration, Instant};

use crate::input;
use crate::pty::Pane;
use crate::theme;

pub struct GridSize {
    pub cols: usize,
    pub lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Default)]
pub struct ViewState {
    selecting: bool,
    last_click: Option<(Instant, Point, u8)>,
    scroll_accum: f32,
    pub search: Option<SearchState>,
    mouse_down_button: Option<u8>,
}

pub struct SearchState {
    pub query: String,
    regex: Option<RegexSearch>,
    pub status: String,
    pub focus_requested: bool,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            regex: None,
            status: String::new(),
            focus_requested: true,
        }
    }
}

/// Things the view could not complete on its own.
#[derive(Default)]
pub struct ViewOutput {
    pub has_focus: bool,
    /// The user sent input to the PTY this frame.
    pub typed: bool,
    pub copy: Option<String>,
    /// A paste that needs the user's confirmation first.
    pub paste_for_review: Option<String>,
    pub open_url: Option<String>,
    /// Typing that belongs in the composer (Claude's own input is hidden).
    /// `Some("")` means "just focus the composer".
    pub redirect: Option<String>,
}

pub struct ViewOptions {
    pub font: FontId,
    pub claude_pane: bool,
    pub option_as_meta: bool,
    pub request_focus: bool,
    /// Keyboard input is consumed by an overlay (palette, dialog).
    pub keyboard_blocked: bool,
    /// Hide Claude Code's input box; the composer replaces it.
    pub hide_claude_input: bool,
}

pub fn cell_size(ctx: &egui::Context, font: &FontId) -> egui::Vec2 {
    ctx.fonts_mut(|f| vec2(f.glyph_width(font, 'M'), f.row_height(font)))
}

pub fn show(ui: &mut egui::Ui, pane: &Pane, st: &mut ViewState, opts: &ViewOptions) -> ViewOutput {
    let mut out = ViewOutput::default();
    let cell = cell_size(ui.ctx(), &opts.font);
    let ppp = ui.ctx().pixels_per_point();
    let outer = ui.available_rect_before_wrap();
    // Breathing room around the grid; the padding is painted as terminal background.
    ui.painter().rect_filled(outer, 0.0, theme::c32(theme::BG));
    let rect = outer.shrink2(vec2(10.0, 0.0)).with_min_y(outer.min.y + 8.0);
    let id = ui.id().with(("term", pane.id));
    let resp = ui.interact(rect, id, Sense::click_and_drag() | Sense::FOCUSABLE);
    ui.advance_cursor_after_rect(outer);
    if opts.request_focus || resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    let focused = resp.has_focus();
    out.has_focus = focused;
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }

    let cols = ((rect.width() / cell.x).floor() as usize).max(2);
    let lines = ((rect.height() / cell.y).floor() as usize).max(1);
    pane.resize(
        cols as u16,
        lines as u16,
        (cell.x * ppp) as u16,
        (cell.y * ppp) as u16,
    );

    let to_point_raw = |pos: egui::Pos2, display_offset: usize| -> (Point, Side) {
        let x = ((pos.x - rect.min.x) / cell.x).clamp(0.0, cols as f32 - 0.001);
        let y = ((pos.y - rect.min.y) / cell.y).clamp(0.0, lines as f32 - 0.001);
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let vp = Point::<usize>::new(y as usize, Column(x as usize));
        (viewport_to_point(display_offset, vp), side)
    };
    let to_point =
        |pos: egui::Pos2, display_offset: usize, hidden: Option<(usize, usize)>| -> (Point, Side) {
            let (mut p, side) = to_point_raw(pos, display_offset);
            // Rows below a collapsed input box are drawn higher than they are.
            if let Some((a, b)) = hidden {
                let drawn = (p.line.0 + display_offset as i32).max(0) as usize;
                if drawn >= a {
                    p.line.0 += (b - a + 1) as i32;
                }
            }
            (p, side)
        };

    let mut term = pane.term.lock();
    let mode = *term.mode();
    let mouse_mode = mode.intersects(TermMode::MOUSE_MODE);
    // Rows of Claude Code's input box, collapsed when the composer replaces it.
    let hidden = (opts.hide_claude_input && opts.claude_pane && term.grid().display_offset() == 0)
        .then(|| claude_input_rows(&term))
        .flatten();
    let gap = hidden.map(|(a, b)| b - a + 1).unwrap_or(0);
    // Screen line -> drawn line (None when hidden).
    let draw_line = |line: usize| -> Option<usize> {
        match hidden {
            Some((a, b)) if (a..=b).contains(&line) => None,
            Some((_, b)) if line > b => Some(line - gap),
            _ => Some(line),
        }
    };

    // ------------------------------------------------------------ mouse
    let mods = ui.input(|i| i.modifiers);
    let report_mouse = mouse_mode && !mods.shift;
    if let Some(pos) = resp.interact_pointer_pos().or(resp.hover_pos()) {
        let display_offset = term.grid().display_offset();
        let (point, side) = to_point(pos, display_offset, hidden);
        let vp_line = (point.line.0 + display_offset as i32).max(0) as usize;
        let (pressed, released, primary_down) = ui.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.any_released(),
                i.pointer.primary_down(),
            )
        });
        let button = ui.input(|i| {
            if i.pointer.button_down(egui::PointerButton::Secondary) {
                2
            } else if i.pointer.button_down(egui::PointerButton::Middle) {
                1
            } else {
                0
            }
        });
        if report_mouse && resp.hovered() {
            if pressed {
                st.mouse_down_button = Some(button);
                pane.write(mouse_report(
                    button,
                    point.column.0,
                    vp_line,
                    true,
                    false,
                    mode,
                    mods,
                ));
            } else if released {
                let b = st.mouse_down_button.take().unwrap_or(0);
                pane.write(mouse_report(
                    b,
                    point.column.0,
                    vp_line,
                    false,
                    false,
                    mode,
                    mods,
                ));
            } else if resp.dragged()
                && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
            {
                let b = st.mouse_down_button.unwrap_or(0);
                pane.write(mouse_report(
                    b,
                    point.column.0,
                    vp_line,
                    true,
                    true,
                    mode,
                    mods,
                ));
            }
        } else if resp.drag_started() && primary_down {
            term.selection = Some(Selection::new(SelectionType::Simple, point, side));
            st.selecting = true;
        } else if st.selecting && resp.dragged() {
            if let Some(sel) = term.selection.as_mut() {
                sel.update(point, side);
            }
        } else if resp.drag_stopped() {
            st.selecting = false;
            if let Some(s) = term.selection_to_string().filter(|s| !s.is_empty())
                && cfg!(target_os = "linux")
            {
                out.copy = Some(s); // primary-selection style copy-on-select
            }
        } else if resp.clicked() {
            let now = Instant::now();
            let count = match st.last_click {
                Some((t, p, n)) if now - t < Duration::from_millis(400) && p == point => {
                    (n % 3) + 1
                }
                _ => 1,
            };
            st.last_click = Some((now, point, count));
            if mods.command {
                out.open_url = link_at(&term, point);
            }
            term.selection = match count {
                2 => Some(Selection::new(SelectionType::Semantic, point, side)),
                3 => Some(Selection::new(SelectionType::Lines, point, side)),
                _ => None,
            };
        }
    }

    // ------------------------------------------------------------ wheel
    if resp.hovered() {
        let delta = ui.input(|i| i.smooth_scroll_delta.y);
        if delta != 0.0 {
            st.scroll_accum += delta / cell.y;
            let n = st.scroll_accum.trunc() as i32;
            st.scroll_accum -= n as f32;
            if n != 0 {
                if report_mouse {
                    let (pt, _) = to_point(resp.hover_pos().unwrap_or(rect.min), 0, hidden);
                    let b = if n > 0 { 64 } else { 65 };
                    for _ in 0..n.abs() {
                        pane.write(mouse_report(
                            b,
                            pt.column.0,
                            pt.line.0.max(0) as usize,
                            true,
                            false,
                            mode,
                            mods,
                        ));
                    }
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                    let seq: &[u8] = if n > 0 { b"\x1bOA" } else { b"\x1bOB" };
                    pane.write(seq.repeat(n.unsigned_abs() as usize));
                } else {
                    term.scroll_display(Scroll::Delta(n));
                }
            }
        }
    }

    // ------------------------------------------------------------ keyboard
    if focused && !opts.keyboard_blocked {
        let events = ui.input(|i| i.events.clone());
        let mut suppress_text = false;
        for ev in events {
            match ev {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.mac_cmd || (modifiers.command && modifiers.shift) {
                        continue; // app shortcuts
                    }
                    // Editing keys belong to the composer while Claude's input is hidden;
                    // Esc and Ctrl combinations (interrupt) still reach Claude.
                    if hidden.is_some()
                        && !modifiers.ctrl
                        && !modifiers.alt
                        && matches!(
                            key,
                            egui::Key::Enter
                                | egui::Key::Backspace
                                | egui::Key::Tab
                                | egui::Key::ArrowUp
                                | egui::Key::ArrowDown
                        )
                    {
                        out.redirect.get_or_insert_with(String::new);
                        continue;
                    }
                    if modifiers.shift && matches!(key, egui::Key::PageUp | egui::Key::PageDown) {
                        let s = if key == egui::Key::PageUp {
                            Scroll::PageUp
                        } else {
                            Scroll::PageDown
                        };
                        term.scroll_display(s);
                        continue;
                    }
                    if let Some(bytes) = input::encode_key(
                        key,
                        modifiers,
                        mode,
                        opts.claude_pane,
                        opts.option_as_meta,
                    ) {
                        pane.write(bytes);
                        out.typed = true;
                        suppress_text = input::suppresses_text(modifiers, opts.option_as_meta);
                    }
                }
                egui::Event::Text(t) => {
                    if !suppress_text && !t.is_empty() {
                        if hidden.is_some() {
                            out.redirect.get_or_insert_with(String::new).push_str(&t);
                        } else {
                            pane.write(t.into_bytes());
                            out.typed = true;
                        }
                    }
                    suppress_text = false;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                    pane.write(t.into_bytes());
                    out.typed = true;
                }
                egui::Event::Copy => {
                    // Linux: Ctrl+C arrives as Copy. Without Shift it is an interrupt.
                    if cfg!(target_os = "linux") && !mods.shift {
                        pane.write(b"\x03".as_slice());
                        out.typed = true;
                    } else if let Some(s) = term.selection_to_string() {
                        out.copy = Some(s);
                    }
                }
                egui::Event::Cut if cfg!(target_os = "linux") && !mods.shift => {
                    pane.write(b"\x18".as_slice());
                }
                egui::Event::Paste(text) if hidden.is_some() => {
                    out.redirect.get_or_insert_with(String::new).push_str(&text);
                }
                egui::Event::Paste(text) => {
                    if cfg!(target_os = "linux") && !mods.shift {
                        // Ctrl+V: let the program read the clipboard itself
                        // (Claude Code uses this to paste images).
                        pane.write(b"\x16".as_slice());
                        continue;
                    }
                    let bracketed = mode.contains(TermMode::BRACKETED_PASTE);
                    if PasteRisk::of(&text).needs_preview(bracketed) {
                        out.paste_for_review = Some(text);
                    } else {
                        pane.write(sanitize::encode_paste(&text, bracketed));
                        out.typed = true;
                    }
                }
                _ => {}
            }
        }
        if out.typed {
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
    }

    // ------------------------------------------------------------ search
    if let Some(search) = st.search.as_mut()
        && search.regex.is_none()
        && !search.query.is_empty()
    {
        match RegexSearch::new(&search.query) {
            Ok(r) => search.regex = Some(r),
            Err(_) => search.status = "invalid pattern".into(),
        }
    }

    // ------------------------------------------------------------ paint
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::c32(theme::BG));
    let content = term.renderable_content();
    let display_offset = content.display_offset;
    let colors = content.colors;
    let selection = content.selection;
    let cursor = content.cursor;
    let snap = |v: f32| (v * ppp).round() / ppp;
    let mut run = String::new();
    let mut run_start = 0usize;
    let mut run_color = Color32::WHITE;
    let mut run_line = usize::MAX;
    let flush = |run: &mut String, line: usize, start: usize, color: Color32| {
        if !run.is_empty() {
            let p = pos2(
                snap(rect.min.x + start as f32 * cell.x),
                snap(rect.min.y + line as f32 * cell.y),
            );
            painter.text(
                p,
                egui::Align2::LEFT_TOP,
                run.as_str(),
                opts.font.clone(),
                color,
            );
            run.clear();
        }
    };
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let c = indexed.cell;
        if c.flags.contains(Flags::WIDE_CHAR_SPACER)
            || c.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let (mut fg, mut bg) = (cell_fg(c.fg, c.flags, colors), theme::resolve(c.bg, colors));
        if c.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = selection.is_some_and(|s| s.contains(indexed.point));
        let width = if c.flags.contains(Flags::WIDE_CHAR) {
            2.0
        } else {
            1.0
        };
        let Some(line) = draw_line(vp.line) else {
            continue;
        };
        let x = rect.min.x + vp.column.0 as f32 * cell.x;
        let y = rect.min.y + line as f32 * cell.y;
        let cell_rect = Rect::from_min_size(
            pos2(snap(x), snap(y)),
            vec2(snap(cell.x * width) + 0.5, snap(cell.y) + 0.5),
        );
        if selected {
            painter.rect_filled(cell_rect, 0.0, theme::SELECTION);
        } else if bg != theme::BG {
            painter.rect_filled(cell_rect, 0.0, theme::c32(bg));
        }
        let mut color = theme::c32(fg);
        if c.flags.contains(Flags::DIM) {
            color = color.gamma_multiply(0.66);
        }
        if c.flags.intersects(Flags::ALL_UNDERLINES) || c.hyperlink().is_some() {
            let uy = snap(y + cell.y - 1.5);
            painter.line_segment(
                [pos2(x, uy), pos2(x + cell.x * width, uy)],
                Stroke::new(1.0, color),
            );
        }
        if c.flags.contains(Flags::STRIKEOUT) {
            let sy = snap(y + cell.y / 2.0);
            painter.line_segment(
                [pos2(x, sy), pos2(x + cell.x * width, sy)],
                Stroke::new(1.0, color),
            );
        }
        let ch = c.c;
        let hidden = c.flags.contains(Flags::HIDDEN) || ch == ' ' || ch == '\0';
        let simple = ch.is_ascii() && width == 1.0;
        // ASCII is batched into runs; anything else is placed per cell so
        // fallback-font glyphs can never shift the grid.
        let continues = simple
            && !hidden
            && run_line == line
            && color == run_color
            && run_start + run.chars().count() == vp.column.0;
        if !continues {
            flush(&mut run, run_line, run_start, run_color);
        }
        if hidden {
            continue;
        }
        if simple {
            if run.is_empty() {
                run_start = vp.column.0;
                run_line = line;
                run_color = color;
            }
            run.push(ch);
        } else {
            let mut s = ch.to_string();
            if let Some(zw) = c.zerowidth() {
                s.extend(zw);
            }
            painter.text(
                pos2(snap(x), snap(y)),
                egui::Align2::LEFT_TOP,
                s,
                opts.font.clone(),
                color,
            );
        }
    }
    flush(&mut run, run_line, run_start, run_color);

    // Cursor
    if mode.contains(TermMode::SHOW_CURSOR)
        && display_offset == 0
        && let Some(vp) = point_to_viewport(display_offset, cursor.point)
        && let Some(cline) = draw_line(vp.line)
    {
        let x = snap(rect.min.x + vp.column.0 as f32 * cell.x);
        let y = snap(rect.min.y + cline as f32 * cell.y);
        let r = Rect::from_min_size(pos2(x, y), cell);
        match (focused, cursor.shape) {
            (_, CursorShape::Hidden) => {}
            (false, _) => {
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0, theme::CURSOR),
                    egui::StrokeKind::Inside,
                );
            }
            (true, CursorShape::Beam) => {
                painter.rect_filled(
                    Rect::from_min_size(r.min, vec2(2.0, cell.y)),
                    0.0,
                    theme::CURSOR,
                );
            }
            (true, CursorShape::Underline) => {
                painter.rect_filled(
                    Rect::from_min_size(pos2(x, y + cell.y - 2.0), vec2(cell.x, 2.0)),
                    0.0,
                    theme::CURSOR,
                );
            }
            (true, _) => {
                painter.rect_filled(r, 0.0, theme::CURSOR.gamma_multiply(0.7));
            }
        }
        // IME candidate window follows the cursor.
        if focused {
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    purpose: Default::default(),
                    rect,
                    cursor_rect: r,
                    should_interrupt_composition: false,
                })
            });
        }
    }
    if display_offset > 0 {
        let label = format!("↑ {display_offset} lines");
        painter.text(
            rect.right_top() + vec2(-8.0, 4.0),
            egui::Align2::RIGHT_TOP,
            label,
            FontId::proportional(11.0),
            theme::MUTED,
        );
    }
    out
}

/// Run a search step over scrollback, selecting and scrolling to the match.
pub fn search_step(pane: &Pane, st: &mut ViewState, forward: bool) {
    let Some(search) = st.search.as_mut() else {
        return;
    };
    if search.regex.is_none() {
        search.regex = RegexSearch::new(&search.query).ok();
    }
    let Some(regex) = search.regex.as_mut() else {
        return;
    };
    let mut term = pane.term.lock();
    let origin = term
        .selection
        .as_ref()
        .and_then(|s| s.to_range(&*term))
        .map(|r| if forward { r.end } else { r.start })
        .unwrap_or_else(|| Point::new(Line(term.screen_lines() as i32 - 1), Column(0)));
    let dir = if forward {
        Direction::Right
    } else {
        Direction::Left
    };
    let found = term.search_next(regex, origin, dir, Side::Left, None);
    match found {
        Some(m) => {
            let (start, end) = (*m.start(), *m.end());
            let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
            sel.update(end, Side::Right);
            term.selection = Some(sel);
            // Scroll so the match is visible.
            let offset = term.grid().display_offset() as i32;
            let top = -offset;
            let bottom = top + term.screen_lines() as i32 - 1;
            if start.line.0 < top || start.line.0 > bottom {
                let target = (-start.line.0 + term.screen_lines() as i32 / 2).max(0);
                term.scroll_display(Scroll::Delta(target - offset));
            }
            search.status = String::new();
        }
        None => search.status = "no matches".into(),
    }
}

pub fn invalidate_search(st: &mut ViewState) {
    if let Some(s) = st.search.as_mut() {
        s.regex = None;
        s.status.clear();
    }
}

/// Plain text of the visible screen (accessibility, grid previews).
pub fn screen_text(pane: &Pane, max_lines: usize) -> Vec<String> {
    let term = pane.term.lock();
    let grid = term.grid();
    let n = term.screen_lines();
    let cols = term.columns();
    let mut lines: Vec<String> = (0..n)
        .map(|l| {
            let row = &grid[Line(l as i32)];
            let s: String = (0..cols)
                .map(|c| &row[Column(c)])
                .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|cell| cell.c)
                .collect();
            s.trim_end().to_string()
        })
        .collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(max_lines);
    lines.split_off(skip)
}

/// Screen rows of Claude Code's prompt box: a horizontal rule (or a rounded
/// box top), one or more prompt lines starting with `❯`/`>`, and a closing
/// rule, near the bottom of the screen. `None` while Claude shows a menu or
/// permission prompt in its place, so those always stay visible.
pub fn claude_input_rows<T>(term: &alacritty_terminal::Term<T>) -> Option<(usize, usize)> {
    let grid = term.grid();
    let n = term.screen_lines();
    let cols = term.columns();
    let row_text = |l: usize| -> String {
        let row = &grid[Line(l as i32)];
        (0..cols).map(|c| row[Column(c)].c).collect::<String>()
    };
    let is_rule = |s: &str| {
        let t = s.trim();
        let rule = t.chars().filter(|c| *c == '─').count();
        rule * 10 >= cols * 6 && t.chars().all(|c| "─╭╮╰╯".contains(c))
    };
    let is_prompt = |s: &str| {
        let t = s.trim_start().trim_start_matches('│').trim_start();
        t.starts_with('❯') || t.starts_with('>')
    };
    let lowest = n.saturating_sub(14);
    for b in (lowest..n).rev() {
        if !is_rule(&row_text(b)) {
            continue;
        }
        for a in (b.saturating_sub(24)..b.saturating_sub(1)).rev() {
            if is_rule(&row_text(a)) {
                return (b - a >= 2 && is_prompt(&row_text(a + 1))).then_some((a, b));
            }
        }
    }
    None
}

fn cell_fg(
    c: Color,
    flags: Flags,
    colors: &alacritty_terminal::term::color::Colors,
) -> alacritty_terminal::vte::ansi::Rgb {
    // Classic behavior: bold renders base colors in their bright variant.
    let c = match c {
        Color::Named(n) if flags.contains(Flags::BOLD) && (n as usize) < 8 => {
            Color::Named(n.to_bright())
        }
        Color::Named(NamedColor::Foreground) if flags.contains(Flags::BOLD) => {
            Color::Named(NamedColor::BrightForeground)
        }
        other => other,
    };
    theme::resolve(c, colors)
}

fn mouse_report(
    button: u8,
    col: usize,
    line: usize,
    pressed: bool,
    motion: bool,
    mode: TermMode,
    m: egui::Modifiers,
) -> Vec<u8> {
    let mut b = button as u32;
    if motion {
        b += 32;
    }
    if m.shift {
        b += 4;
    }
    if m.alt {
        b += 8;
    }
    if m.ctrl {
        b += 16;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        format!(
            "\x1b[<{b};{};{}{}",
            col + 1,
            line + 1,
            if pressed { 'M' } else { 'm' }
        )
        .into_bytes()
    } else {
        let b = if pressed { b } else { 3 + (b & !3) };
        let enc = |v: usize| (32 + v + 1).min(255) as u8;
        vec![
            0x1b,
            b'[',
            b'M',
            (32 + b).min(255) as u8,
            enc(col),
            enc(line),
        ]
    }
}

/// OSC 8 hyperlink at a point, or a URL / path detected in the line text.
fn link_at<T>(term: &alacritty_terminal::Term<T>, p: Point) -> Option<String> {
    let row = &term.grid()[p.line];
    if let Some(h) = row[p.column].hyperlink() {
        return Some(h.uri().to_string());
    }
    let cols = term.columns();
    let chars: Vec<char> = (0..cols).map(|c| row[Column(c)].c).collect();
    let is_sep = |c: char| c.is_whitespace() || "\"'<>()[]{}`".contains(c);
    let i = p.column.0.min(cols.saturating_sub(1));
    if is_sep(chars[i]) {
        return None;
    }
    let start = (0..=i).rev().take_while(|&k| !is_sep(chars[k])).last()?;
    let end = (i..cols).take_while(|&k| !is_sep(chars[k])).last()?;
    let word: String = chars[start..=end].iter().collect();
    let word = word.trim_end_matches(['.', ',', ':', ';']).to_string();
    let is_url = ["http://", "https://", "file://", "mailto:"]
        .iter()
        .any(|s| word.starts_with(s));
    let is_path = word.starts_with('/') || word.starts_with("~/") || word.starts_with("./");
    (is_url || is_path).then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    fn term_with(lines: &[&str]) -> Term<VoidListener> {
        let mut t = Term::new(
            Config::default(),
            &GridSize {
                cols: 40,
                lines: 10,
            },
            VoidListener,
        );
        let mut p: Processor = Processor::new();
        let body = lines.join("\r\n");
        p.advance(&mut t, body.as_bytes());
        t
    }

    #[test]
    fn finds_claude_input_box() {
        let rule = "─".repeat(40);
        let t = term_with(&[
            "⏺ Done.",
            "",
            &rule,
            "❯ push and tag",
            &rule,
            "  Model: Opus 5.5 | Ctx: 865k",
        ]);
        assert_eq!(claude_input_rows(&t), Some((2, 4)));
        // Multi-line input and the older rounded box.
        let top = format!("╭{}╮", "─".repeat(38));
        let bot = format!("╰{}╯", "─".repeat(38));
        let t = term_with(&[
            "x",
            &top,
            "│ > first line",
            "│   second line",
            &bot,
            "  ? for shortcuts",
        ]);
        assert_eq!(claude_input_rows(&t), Some((1, 4)));
    }

    #[test]
    fn leaves_menus_and_permission_prompts_visible() {
        let rule = "─".repeat(40);
        let t = term_with(&[
            &rule,
            " Bash command",
            "   curl -sI https://example.com",
            " Do you want to proceed?",
            " ❯ 1. Yes",
            "   2. No",
            &rule,
        ]);
        assert_eq!(
            claude_input_rows(&t),
            None,
            "the line after the top rule isn't a prompt"
        );
        let t = term_with(&["$ ls", "a b c"]);
        assert_eq!(claude_input_rows(&t), None);
    }
}
