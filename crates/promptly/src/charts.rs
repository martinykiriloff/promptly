//! Small live charts painted with egui: sparklines, limit meters and an
//! hourly bar chart. Thin marks, recessive axes, text in text colors;
//! status colors only for limit levels and always paired with a label.

use egui::{Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use promptly_core::statusline::LimitWindow;
use promptly_core::usage::{fmt_tokens, fmt_until};

use crate::theme::{self, tokens as t};

/// Smoothly animate a number toward its target unless motion is reduced.
pub fn tween(
    ui: &Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    target: f32,
    reduce_motion: bool,
) -> f32 {
    if reduce_motion {
        return target;
    }
    let v = ui
        .ctx()
        .animate_value_with_time(egui::Id::new(id), target, 0.6);
    // Never show "-0.00" while easing from zero.
    if v.abs() < 0.005 { 0.0 } else { v }
}

/// Level of a plan limit: label + color, so color is never the only cue.
pub fn limit_level(pct: f32) -> (&'static str, Color32) {
    if pct >= 90.0 {
        ("Near limit", theme::RED)
    } else if pct >= 70.0 {
        ("High", theme::AMBER)
    } else {
        ("OK", t::TEXT_1)
    }
}

/// Horizontal meter for a plan limit, 6 px track with a rounded fill.
pub fn limit_bar(ui: &mut Ui, frac: f32, color: Color32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), t::BORDER);
    let mut fill = rect;
    fill.set_width((rect.width() * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 6.0 } else { 0.0 }));
    ui.painter().rect_filled(fill, CornerRadius::same(3), color);
}

/// Compact limit row for the sidebar: "5-hour  42%  ▬▬▬───  resets 2h 14m".
pub fn limit_row(ui: &mut Ui, label: &str, w: Option<LimitWindow>, now: i64, reduce_motion: bool) {
    let width = ui.available_width();
    match w {
        None => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).size(12.0).color(t::TEXT_2));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("—").size(11.0).color(t::TEXT_3));
                });
            });
            limit_bar(ui, 0.0, t::BORDER, width);
        }
        Some(w) => {
            let pct = tween(ui, ("limit", label), w.used_percentage, reduce_motion);
            let (level, color) = limit_level(w.used_percentage);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).size(12.0).color(t::TEXT_2));
                ui.label(
                    egui::RichText::new(format!("{pct:.0}%"))
                        .size(12.0)
                        .color(t::TEXT),
                );
                if level != "OK" {
                    ui.label(egui::RichText::new(level).size(11.0).color(color));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("resets {}", fmt_until(w.resets_at - now)))
                            .size(11.0)
                            .color(t::TEXT_3),
                    );
                });
            });
            let fill = if level == "OK" { t::TEXT_2 } else { color };
            limit_bar(ui, pct / 100.0, fill, width);
        }
    }
}

/// Line sparkline with a soft area fill and an end marker. `y_max` fixes the
/// scale (e.g. 100 for percentages); `None` scales to the data.
pub fn sparkline(
    ui: &mut Ui,
    points: &[(f64, f64)],
    x_range: (f64, f64),
    y_max: Option<f64>,
    size: egui::Vec2,
    color: Color32,
    tooltip: impl Fn(f64, f64) -> String,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect.expand(4.0));
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, t::BORDER),
    );
    if points.len() < 2 {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "collecting…",
            FontId::proportional(10.5),
            t::TEXT_3,
        );
        return resp;
    }
    let (x0, x1) = x_range;
    let ymax = y_max
        .unwrap_or_else(|| points.iter().map(|p| p.1).fold(0.0, f64::max))
        .max(1e-9);
    let ymin = if y_max.is_some() {
        0.0
    } else {
        points
            .iter()
            .map(|p| p.1)
            .fold(f64::MAX, f64::min)
            .min(ymax)
    };
    let span = (ymax - ymin).max(1e-9);
    let to = |x: f64, y: f64| {
        pos2(
            rect.min.x + ((x - x0) / (x1 - x0).max(1e-9)) as f32 * rect.width(),
            rect.max.y - ((y - ymin) / span) as f32 * (rect.height() - 2.0),
        )
    };
    let line: Vec<_> = points.iter().map(|&(x, y)| to(x, y)).collect();
    // Area under the line as per-segment quads (convex for monotone x).
    for w in line.windows(2) {
        let quad = vec![
            w[0],
            w[1],
            pos2(w[1].x, rect.max.y),
            pos2(w[0].x, rect.max.y),
        ];
        p.add(egui::Shape::convex_polygon(
            quad,
            color.gamma_multiply(0.12),
            Stroke::NONE,
        ));
    }
    p.add(egui::Shape::line(line.clone(), Stroke::new(1.5, color)));
    let end = *line.last().unwrap();
    p.circle_filled(end, 3.5, t::BG_SIDEBAR);
    p.circle_filled(end, 2.5, color);
    // Crosshair + tooltip on hover.
    if let Some(h) = resp.hover_pos() {
        let fx = x0 + ((h.x - rect.min.x) / rect.width()) as f64 * (x1 - x0);
        if let Some(&(x, y)) = points
            .iter()
            .min_by(|a, b| (a.0 - fx).abs().total_cmp(&(b.0 - fx).abs()))
        {
            let at = to(x, y);
            p.line_segment(
                [pos2(at.x, rect.min.y), pos2(at.x, rect.max.y)],
                Stroke::new(1.0, t::BORDER_STRONG),
            );
            p.circle_filled(at, 3.0, color);
            return resp.on_hover_text_at_pointer(tooltip(x, y));
        }
    }
    resp
}

/// 24 hourly bars for today's tokens; the current hour is emphasized.
pub fn hourly_bars(ui: &mut Ui, hours: &[u64; 24], current_hour: usize, size: egui::Vec2) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let label_h = 14.0;
    let plot = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - label_h));
    let max = hours.iter().copied().max().unwrap_or(0).max(1) as f32;
    let gap = 2.0;
    let bw = (plot.width() - gap * 23.0) / 24.0;
    p.line_segment(
        [plot.left_bottom(), plot.right_bottom()],
        Stroke::new(1.0, t::BORDER),
    );
    let mut hovered = None;
    for (h, &v) in hours.iter().enumerate() {
        let x = plot.min.x + h as f32 * (bw + gap);
        let col_rect = Rect::from_min_max(pos2(x, plot.min.y), pos2(x + bw, plot.max.y));
        let height = (v as f32 / max) * plot.height();
        if v > 0 {
            let bar = Rect::from_min_max(
                pos2(x, plot.max.y - height.max(2.0)),
                pos2(x + bw, plot.max.y),
            );
            let color = if h == current_hour {
                theme::BLUE
            } else {
                theme::BLUE.gamma_multiply(0.45)
            };
            p.rect_filled(
                bar,
                CornerRadius {
                    nw: 2,
                    ne: 2,
                    sw: 0,
                    se: 0,
                },
                color,
            );
        }
        if resp.hover_pos().is_some_and(|hp| col_rect.contains(hp)) {
            hovered = Some((h, v));
            p.rect_filled(
                col_rect,
                CornerRadius::same(2),
                Color32::from_white_alpha(8),
            );
        }
        if h % 6 == 0 {
            p.text(
                pos2(x, rect.max.y),
                egui::Align2::LEFT_BOTTOM,
                format!("{h:02}:00"),
                FontId::proportional(10.0),
                t::TEXT_3,
            );
        }
    }
    if let Some((h, v)) = hovered {
        resp.on_hover_text_at_pointer(format!(
            "{h:02}:00–{:02}:00\n{} tokens",
            (h + 1) % 24,
            fmt_tokens(v)
        ));
    }
}

/// Labelled number tile.
pub fn stat_tile(ui: &mut Ui, label: &str, value: &str, detail: &str, width: f32) {
    egui::Frame::new()
        .fill(t::BG_SIDEBAR)
        .stroke(Stroke::new(1.0, t::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width - 30.0);
                ui.label(egui::RichText::new(label).size(12.0).color(t::TEXT_2));
                ui.add(
                    egui::Label::new(egui::RichText::new(value).size(24.0).color(t::TEXT))
                        .truncate(),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(detail).size(11.5).color(t::TEXT_3))
                        .truncate(),
                );
            });
        });
}
