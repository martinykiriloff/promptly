//! Small design system: vector icons, buttons, rows, pills and keycaps.
//! Everything is painted directly so hit targets cover the whole control
//! and text never steals clicks.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use crate::theme::tokens as t;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Sparkle,
    Terminal,
    Clock,
    Grid,
    Sliders,
    PanelRight,
    Compose,
    Branch,
    Close,
    Fork,
    Search,
    Send,
    Refresh,
    Chart,
    Thought,
}

/// Paint an icon centered in `rect` with 1.5 px strokes on a 16 px grid.
pub fn paint_icon(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let p = ui.painter();
    let s = rect.width().min(rect.height()) / 16.0;
    let c = rect.center();
    let at = |x: f32, y: f32| pos2(c.x + (x - 8.0) * s, c.y + (y - 8.0) * s);
    let st = Stroke::new(1.5 * s.max(1.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], st);
    match icon {
        Icon::Sparkle => {
            // Claude-like burst: four long and four short rays.
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let r = if i % 2 == 0 { 6.0 } else { 4.0 };
                line(
                    (8.0 + a.cos() * 1.6, 8.0 + a.sin() * 1.6),
                    (8.0 + a.cos() * r, 8.0 + a.sin() * r),
                );
            }
        }
        Icon::Terminal => {
            p.rect_stroke(
                Rect::from_min_max(at(2.0, 3.0), at(14.0, 13.0)),
                2.0 * s,
                st,
                egui::StrokeKind::Middle,
            );
            line((5.0, 6.5), (7.0, 8.0));
            line((7.0, 8.0), (5.0, 9.5));
            line((8.5, 10.0), (11.0, 10.0));
        }
        Icon::Clock => {
            p.circle_stroke(c, 5.8 * s, st);
            line((8.0, 5.0), (8.0, 8.0));
            line((8.0, 8.0), (10.2, 9.4));
        }
        Icon::Grid => {
            for (x, y) in [(2.5, 2.5), (9.0, 2.5), (2.5, 9.0), (9.0, 9.0)] {
                p.rect_stroke(
                    Rect::from_min_max(at(x, y), at(x + 4.5, y + 4.5)),
                    1.2 * s,
                    st,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Icon::Sliders => {
            // Gear: ring, hub and eight teeth.
            p.circle_stroke(c, 4.2 * s, st);
            p.circle_stroke(c, 1.6 * s, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                line(
                    (8.0 + a.cos() * 4.6, 8.0 + a.sin() * 4.6),
                    (8.0 + a.cos() * 6.4, 8.0 + a.sin() * 6.4),
                );
            }
        }
        Icon::PanelRight => {
            p.rect_stroke(
                Rect::from_min_max(at(2.0, 3.0), at(14.0, 13.0)),
                2.0 * s,
                st,
                egui::StrokeKind::Middle,
            );
            line((9.5, 3.0), (9.5, 13.0));
        }
        Icon::Compose => {
            p.rect_stroke(
                Rect::from_min_max(at(2.0, 4.0), at(14.0, 13.0)),
                2.0 * s,
                st,
                egui::StrokeKind::Middle,
            );
            line((5.0, 7.5), (11.0, 7.5));
            line((5.0, 10.0), (8.5, 10.0));
        }
        Icon::Branch => {
            p.circle_stroke(at(5.0, 3.8), 1.6 * s, st);
            p.circle_stroke(at(5.0, 12.2), 1.6 * s, st);
            p.circle_stroke(at(11.0, 5.5), 1.6 * s, st);
            line((5.0, 5.4), (5.0, 10.6));
            let pts = [at(11.0, 7.1), at(11.0, 8.6), at(5.0, 10.2)];
            p.add(egui::Shape::line(pts.to_vec(), st));
        }
        Icon::Close => {
            line((4.0, 4.0), (12.0, 12.0));
            line((12.0, 4.0), (4.0, 12.0));
        }
        Icon::Fork => {
            p.circle_stroke(at(8.0, 13.0), 1.6 * s, st);
            p.circle_stroke(at(3.5, 3.5), 1.6 * s, st);
            p.circle_stroke(at(8.0, 3.5), 1.6 * s, st);
            p.circle_stroke(at(12.5, 3.5), 1.6 * s, st);
            line((8.0, 5.1), (8.0, 11.4));
            line((3.5, 5.1), (8.0, 9.0));
            line((12.5, 5.1), (8.0, 9.0));
        }
        Icon::Search => {
            p.circle_stroke(at(7.0, 7.0), 4.3 * s, st);
            line((10.2, 10.2), (13.5, 13.5));
        }
        Icon::Send => {
            line((8.0, 13.0), (8.0, 3.5));
            line((4.0, 7.5), (8.0, 3.5));
            line((12.0, 7.5), (8.0, 3.5));
        }
        Icon::Thought => {
            // Thought bubble: a rounded cloud with two trailing dots.
            p.rect_stroke(
                Rect::from_min_max(at(2.0, 2.5), at(14.0, 10.5)),
                4.0 * s,
                st,
                egui::StrokeKind::Middle,
            );
            p.circle_filled(at(5.0, 12.6), 1.2 * s, color);
            p.circle_filled(at(3.2, 14.4), 0.8 * s, color);
            for x in [5.5, 8.0, 10.5] {
                p.circle_filled(at(x, 6.5), 0.9 * s, color);
            }
        }
        Icon::Chart => {
            line((2.5, 13.5), (13.5, 13.5));
            for (x, h) in [(4.5, 4.0), (8.0, 8.5), (11.5, 6.0)] {
                p.rect_filled(
                    Rect::from_min_max(at(x - 1.2, 13.5 - h), at(x + 1.2, 13.0)),
                    CornerRadius::same(1),
                    color,
                );
            }
        }
        Icon::Refresh => {
            let pts: Vec<_> = (0..=20)
                .map(|i| {
                    let a = -0.6 + i as f32 / 20.0 * 5.0;
                    at(8.0 + a.cos() * 5.0, 8.0 + a.sin() * 5.0)
                })
                .collect();
            p.add(egui::Shape::line(pts, st));
            line((12.5, 2.8), (12.6, 5.6));
            line((12.6, 5.6), (9.8, 5.4));
        }
    }
}

/// Square, frameless icon button with hover/active backgrounds.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let bg = if active {
        t::ACTIVE
    } else if resp.hovered() {
        t::HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
    let fg = if active || resp.hovered() {
        t::TEXT
    } else {
        t::TEXT_2
    };
    paint_icon(ui, rect.shrink(6.0), icon, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Filled accent button. `full_width` stretches to the available width.
pub fn primary_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    shortcut: Option<&str>,
    full_width: bool,
) -> Response {
    let font = FontId::proportional(13.0);
    let text_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.into(), font.clone(), t::TEXT)
            .size()
            .x
    });
    let sc_w = shortcut
        .map(|sc| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(sc.into(), FontId::proportional(11.5), t::TEXT)
                    .size()
                    .x
            }) + 12.0
        })
        .unwrap_or(0.0);
    let w = if full_width {
        ui.available_width()
    } else {
        text_w + sc_w + if icon.is_some() { 44.0 } else { 24.0 }
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t::ACCENT_PRESSED
    } else if resp.hovered() {
        t::ACCENT_HOVER
    } else {
        t::ACCENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    let mut x = rect.min.x + 12.0;
    if let Some(i) = icon {
        paint_icon(
            ui,
            Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)),
            i,
            Color32::WHITE,
        );
        x += 20.0;
    }
    ui.painter().text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        Color32::WHITE,
    );
    if let Some(sc) = shortcut {
        ui.painter().text(
            pos2(rect.max.x - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            sc,
            FontId::proportional(11.5),
            Color32::from_white_alpha(170),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Quiet text button: icon + label, hover background only.
pub fn ghost_button(ui: &mut Ui, icon: Icon, label: &str, shortcut: Option<&str>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), t::HOVER);
    }
    let fg = if resp.hovered() { t::TEXT } else { t::TEXT_2 };
    paint_icon(
        ui,
        Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(14.0, 14.0)),
        icon,
        fg,
    );
    ui.painter().text(
        pos2(rect.min.x + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        fg,
    );
    if let Some(sc) = shortcut {
        ui.painter().text(
            pos2(rect.max.x - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            sc,
            FontId::proportional(11.5),
            t::TEXT_3,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Uppercase-free section label with an optional count badge.
pub fn section_header(ui: &mut Ui, title: &str, count: Option<usize>, badge_color: Color32) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(title).size(11.5).color(t::TEXT_3));
        if let Some(n) = count.filter(|n| *n > 0) {
            pill(ui, &n.to_string(), badge_color);
        }
    });
    ui.add_space(2.0);
}

/// Tinted rounded label, e.g. a state or a count.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, color));
    let size = vec2(galley.size().x + 12.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.16));
    ui.painter().galley(
        pos2(rect.min.x + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// State pill with a leading dot: "● Working".
pub fn state_pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, color));
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 22.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), color.gamma_multiply(0.14));
    ui.painter()
        .circle_filled(pos2(rect.min.x + 11.0, rect.center().y), 3.5, color);
    ui.painter().galley(
        pos2(rect.min.x + 19.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// Keycap for shortcut hints.
pub fn kbd(ui: &mut Ui, text: &str) {
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, t::TEXT_2));
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 10.0, 18.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        t::BG_ELEVATED_2,
        Stroke::new(1.0, t::BORDER),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(rect.min.x + 5.0, rect.center().y - galley.size().y / 2.0),
        galley,
        t::TEXT_2,
    );
}

/// Thin horizontal meter, e.g. context window usage.
pub fn meter(ui: &mut Ui, frac: f32, color: Color32, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let bar = Rect::from_center_size(rect.center(), vec2(width, 4.0));
    ui.painter()
        .rect_filled(bar, CornerRadius::same(2), t::BORDER);
    let mut fill = bar;
    fill.set_width(bar.width() * frac.clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, CornerRadius::same(2), color);
    resp
}

pub struct RowSpec<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub dot: Color32,
    pub icon: Option<Icon>,
    pub trailing: Option<&'a str>,
    pub active: bool,
    /// Draws an emphasized left edge (used for "needs you" items).
    pub accent: Option<Color32>,
}

/// Two-line list row; the whole rect is the click target.
pub fn row(ui: &mut Ui, spec: RowSpec<'_>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if spec.active {
        t::ACTIVE
    } else if resp.hovered() {
        t::HOVER
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(8), bg);
    if let Some(a) = spec.accent {
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.min.x, rect.min.y + 8.0),
                vec2(3.0, rect.height() - 16.0),
            ),
            CornerRadius::same(2),
            a,
        );
    }
    let left = rect.min.x + 14.0;
    match spec.icon {
        Some(i) => {
            let r = Rect::from_center_size(pos2(left + 6.0, rect.min.y + 15.0), vec2(14.0, 14.0));
            paint_icon(ui, r, i, if spec.active { t::TEXT } else { t::TEXT_2 });
            painter.circle_filled(pos2(left + 12.0, rect.min.y + 21.0), 3.6, bg_or(bg));
            painter.circle_filled(pos2(left + 12.0, rect.min.y + 21.0), 2.6, spec.dot);
        }
        None => {
            painter.circle_filled(pos2(left + 6.0, rect.min.y + 15.0), 4.0, spec.dot);
        }
    }
    let text_x = left + 22.0;
    let trailing_w = spec
        .trailing
        .map(|s| s.len() as f32 * 6.5 + 12.0)
        .unwrap_or(0.0);
    let max_w = (rect.max.x - text_x - 10.0 - trailing_w).max(20.0);
    let title_col = if spec.active { t::TEXT } else { t::TEXT_1 };
    let title = elide(ui, spec.title, FontId::proportional(13.0), title_col, max_w);
    painter.galley(pos2(text_x, rect.min.y + 6.0), title, title_col);
    let sub = elide(
        ui,
        spec.subtitle,
        FontId::proportional(11.5),
        t::TEXT_3,
        max_w + trailing_w,
    );
    painter.galley(pos2(text_x, rect.min.y + 24.0), sub, t::TEXT_3);
    if let Some(tr) = spec.trailing {
        painter.text(
            pos2(rect.max.x - 10.0, rect.min.y + 14.0),
            Align2::RIGHT_CENTER,
            tr,
            FontId::proportional(11.0),
            t::TEXT_3,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Ring color that "cuts out" the status dot from the row background.
fn bg_or(bg: Color32) -> Color32 {
    if bg == Color32::TRANSPARENT {
        t::BG_SIDEBAR
    } else {
        bg
    }
}

/// Single-line galley truncated with an ellipsis.
pub fn elide(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: max_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Rounded card frame for floating surfaces (palette, dialogs, toasts).
pub fn floating_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(t::BG_ELEVATED)
        .stroke(Stroke::new(1.0, t::BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .shadow(egui::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(150),
        })
}

pub fn state_label(s: promptly_core::state::SessionState) -> &'static str {
    use promptly_core::state::SessionState as S;
    match s {
        S::Starting => "Starting",
        S::Working => "Working",
        S::WaitingPermission => "Needs approval",
        S::WaitingInput => "Waiting for you",
        S::Idle => "Ready",
        S::Done => "Done",
        S::Error => "Error",
        S::Exited => "Exited",
    }
}
