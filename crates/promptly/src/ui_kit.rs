//! Promptly's design kit. One icon family (Lucide), one set of radii
//! (7 controls, 10 cards, 14 sheets), eased hover states, and controls that
//! feel at home on macOS. Everything is painted directly so hit targets
//! cover the whole control and text never steals clicks.

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke, Ui, pos2,
    vec2,
};

use crate::theme::tokens as t;

/// Font family holding the Lucide icon glyphs (registered in `fonts.rs`).
pub const ICON_FAMILY: &str = "icons";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    Plus,
    Quote,
    External,
    Check,
    ChevronDown,
    ChevronRight,
    Eye,
    Image,
    FileText,
    Film,
    File,
    ChevronsUpDown,
    UserPlus,
    Users,
}

impl Icon {
    /// Lucide code point (lucide-static 0.544.0).
    pub fn glyph(self) -> char {
        let code = match self {
            Icon::Sparkle => 0xe416,
            Icon::Terminal => 0xe181,
            Icon::Clock => 0xe1f5, // history
            Icon::Grid => 0xe0ff,
            Icon::Sliders => 0xe29a,
            Icon::PanelRight => 0xe435,
            Icon::Compose => 0xe172,
            Icon::Branch => 0xe0e2,
            Icon::Close => 0xe1b2,
            Icon::Fork => 0xe28d,
            Icon::Search => 0xe151,
            Icon::Send => 0xe04a, // arrow-up
            Icon::Refresh => 0xe145,
            Icon::Chart => 0xe2a3,
            Icon::Thought => 0xe3ca, // brain
            Icon::Plus => 0xe13d,
            Icon::Quote => 0xe239,
            Icon::External => 0xe0b9,
            Icon::Check => 0xe06c,
            Icon::ChevronDown => 0xe06d,
            Icon::ChevronRight => 0xe06f,
            Icon::Eye => 0xe0ba,
            Icon::Image => 0xe0f6,
            Icon::FileText => 0xe0cc,
            Icon::Film => 0xe0d0,
            Icon::File => 0xe0c0,
            Icon::ChevronsUpDown => 0xe211,
            Icon::UserPlus => 0xe470,
            Icon::Users => 0xe472,
        };
        char::from_u32(code).unwrap_or('?')
    }
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICON_FAMILY.into()))
}

/// Paint an icon centred in `rect`, sized to the rect's smaller side.
pub fn paint_icon(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon.glyph(),
        icon_font(size),
        color,
    );
}

/// Eased 0..1 hover amount for a control (120 ms).
pub fn hover_t(ui: &Ui, resp: &Response) -> f32 {
    ui.ctx()
        .animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12)
}

pub fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

fn text_w(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.into(), font, t::TEXT).size().x)
}

/// Square, frameless icon button with an eased hover.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let h = hover_t(ui, &resp);
    let bg = if active {
        t::ACTIVE
    } else {
        mix(Color32::TRANSPARENT, t::HOVER, h)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    let fg = if active {
        t::TEXT
    } else {
        mix(t::TEXT_2, t::TEXT, h)
    };
    paint_icon(ui, rect.shrink(7.0), icon, fg);
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
    let sc_font = FontId::proportional(11.5);
    let sc_w = shortcut
        .map(|sc| text_w(ui, sc, sc_font.clone()) + 12.0)
        .unwrap_or(0.0);
    let w = if full_width {
        ui.available_width()
    } else {
        text_w(ui, label, font.clone()) + sc_w + if icon.is_some() { 46.0 } else { 26.0 }
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    let h = hover_t(ui, &resp);
    let enabled = ui.is_enabled();
    let fill = if !enabled {
        t::BG_ELEVATED_2
    } else if resp.is_pointer_button_down_on() {
        t::ACCENT_PRESSED
    } else {
        mix(t::ACCENT, t::ACCENT_HOVER, h)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    if enabled {
        // Hairline top highlight, like a native push button.
        ui.painter().line_segment(
            [
                rect.left_top() + vec2(6.0, 0.5),
                rect.right_top() + vec2(-6.0, 0.5),
            ],
            Stroke::new(1.0, Color32::from_white_alpha(36)),
        );
    }
    let fg = if enabled { Color32::WHITE } else { t::TEXT_3 };
    let mut x = rect.min.x + 13.0;
    if let Some(i) = icon {
        paint_icon(
            ui,
            Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)),
            i,
            fg,
        );
        x += 21.0;
    }
    ui.painter().text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    if let Some(sc) = shortcut {
        ui.painter().text(
            pos2(rect.max.x - 11.0, rect.center().y),
            Align2::RIGHT_CENTER,
            sc,
            sc_font,
            Color32::from_white_alpha(150),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Quiet bordered button for secondary actions.
pub fn secondary_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> Response {
    let font = FontId::proportional(12.5);
    let w = text_w(ui, label, font.clone()) + if icon.is_some() { 42.0 } else { 24.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    let h = hover_t(ui, &resp);
    ui.painter().rect(
        rect,
        CornerRadius::same(7),
        mix(t::BG_ELEVATED, t::BG_ELEVATED_2, h),
        Stroke::new(1.0, mix(t::BORDER, t::BORDER_STRONG, h)),
        egui::StrokeKind::Inside,
    );
    let fg = mix(t::TEXT_1, t::TEXT, h);
    let mut x = rect.min.x + 12.0;
    if let Some(i) = icon {
        paint_icon(
            ui,
            Rect::from_center_size(pos2(x + 6.5, rect.center().y), vec2(13.0, 13.0)),
            i,
            fg,
        );
        x += 19.0;
    }
    ui.painter().text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Quiet sidebar action row: icon + label; the shortcut fades in on hover.
pub fn ghost_button(ui: &mut Ui, icon: Icon, label: &str, shortcut: Option<&str>) -> Response {
    nav_row(
        ui,
        icon,
        label,
        shortcut,
        mix(t::TEXT_2, t::TEXT_1, 0.0),
        false,
    )
}

/// The primary "new" row: same shape as the others, accent icon.
pub fn new_row(ui: &mut Ui, label: &str, shortcut: Option<&str>) -> Response {
    nav_row(ui, Icon::Plus, label, shortcut, t::ACCENT_HOVER, true)
}

fn nav_row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    shortcut: Option<&str>,
    icon_col: Color32,
    strong: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let h = hover_t(ui, &resp);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        mix(Color32::TRANSPARENT, t::HOVER, h),
    );
    let text_col = if strong {
        t::TEXT
    } else {
        mix(t::TEXT_2, t::TEXT, h)
    };
    let ic = if strong {
        icon_col
    } else {
        mix(t::TEXT_3, t::TEXT_1, h)
    };
    paint_icon(
        ui,
        Rect::from_center_size(pos2(rect.min.x + 17.0, rect.center().y), vec2(14.0, 14.0)),
        icon,
        ic,
    );
    ui.painter().text(
        pos2(rect.min.x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        text_col,
    );
    if let Some(sc) = shortcut
        && h > 0.01
    {
        ui.painter().text(
            pos2(rect.max.x - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            sc,
            FontId::proportional(11.0),
            t::TEXT_3.gamma_multiply(h),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Small uppercase section label with an optional count.
pub fn section_header(ui: &mut Ui, title: &str, count: Option<usize>, badge_color: Color32) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .size(10.5)
                .color(t::TEXT_3),
        );
        if let Some(n) = count.filter(|n| *n > 0) {
            ui.label(
                egui::RichText::new(n.to_string())
                    .size(10.5)
                    .color(badge_color),
            );
        }
    });
    ui.add_space(4.0);
}

/// Tinted rounded label, e.g. a state or a count.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, color));
    let size = vec2(galley.size().x + 12.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), color.gamma_multiply(0.11));
    ui.painter().galley(
        pos2(rect.min.x + 7.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// Session state as a coloured dot and a quiet label ("● Working").
pub fn state_pill(ui: &mut Ui, label: &str, color: Color32) -> Response {
    dot_label(ui, label, color, t::TEXT_2)
}

/// A small coloured dot followed by text.
pub fn dot_label(ui: &mut Ui, label: &str, dot: Color32, text: Color32) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, text));
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 13.0, 20.0), Sense::hover());
    ui.painter()
        .circle_filled(pos2(rect.min.x + 3.5, rect.center().y), 3.0, dot);
    ui.painter().galley(
        pos2(rect.min.x + 13.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
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
        CornerRadius::same(5),
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

/// macOS-style switch with an eased knob.
pub fn toggle_switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("knob"), *on, 0.16);
    let h = hover_t(ui, &resp);
    let off = mix(t::BORDER_STRONG, Color32::from_rgb(0x4a, 0x4b, 0x54), h);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), mix(off, t::ACCENT, k));
    let x = egui::lerp((rect.min.x + 10.0)..=(rect.max.x - 10.0), k);
    ui.painter().circle_filled(
        pos2(x, rect.center().y + 0.6),
        8.2,
        Color32::from_black_alpha(60),
    );
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    let value = *on;
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, value, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded group of settings rows (System Settings style).
pub fn settings_group(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui)) {
    if let Some(title) = title {
        ui.label(
            egui::RichText::new(title)
                .size(12.0)
                .strong()
                .color(t::TEXT_2),
        );
        ui.add_space(6.0);
    }
    egui::Frame::new()
        .fill(t::BG_ELEVATED)
        .stroke(Stroke::new(1.0, t::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    ui.add_space(18.0);
}

/// One settings row: label (+ optional detail) on the left, control on the
/// right. Rows after the first draw a hairline separator above themselves.
pub fn setting_row<R>(
    ui: &mut Ui,
    first: bool,
    label: &str,
    detail: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    if !first {
        let y = ui.cursor().min.y;
        let r = ui.max_rect();
        ui.painter().line_segment(
            [pos2(r.min.x, y), pos2(r.max.x, y)],
            Stroke::new(1.0, t::BORDER),
        );
    }
    let h = if detail.is_empty() { 42.0 } else { 54.0 };
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(w, h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(vec2(w, h));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                // Centre the text block: 17 pt label, 2 gap, 15 pt detail.
                let block = if detail.is_empty() { 17.0 } else { 34.0 };
                ui.add_space(((h - block) / 2.0).max(0.0));
                ui.label(egui::RichText::new(label).size(13.0).color(t::TEXT));
                if !detail.is_empty() {
                    ui.label(egui::RichText::new(detail).size(11.5).color(t::TEXT_3));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control)
                .inner
        },
    )
    .inner
}

/// Sidebar navigation item for sheets (Settings categories).
pub fn nav_item(ui: &mut Ui, icon: Icon, label: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let h = hover_t(ui, &resp);
    let bg = if active {
        t::ACTIVE
    } else {
        mix(Color32::TRANSPARENT, t::HOVER, h)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    let fg = if active {
        t::TEXT
    } else {
        mix(t::TEXT_1, t::TEXT, h)
    };
    let ic = if active {
        t::ACCENT_HOVER
    } else {
        mix(t::TEXT_2, t::TEXT, h)
    };
    paint_icon(
        ui,
        Rect::from_center_size(pos2(rect.min.x + 17.0, rect.center().y), vec2(15.0, 15.0)),
        icon,
        ic,
    );
    ui.painter().text(
        pos2(rect.min.x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Compact − value + stepper.
pub fn stepper(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    unit: &str,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(112.0, 28.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(7),
        t::BG_ELEVATED_2,
        Stroke::new(1.0, t::BORDER),
        egui::StrokeKind::Inside,
    );
    let mut changed = false;
    for (i, (glyph, delta)) in [("−", -1.0f32), ("+", 1.0)].into_iter().enumerate() {
        let r = if i == 0 {
            Rect::from_min_size(rect.min, vec2(30.0, rect.height()))
        } else {
            Rect::from_min_size(
                pos2(rect.max.x - 30.0, rect.min.y),
                vec2(30.0, rect.height()),
            )
        };
        let resp = ui.interact(r, ui.id().with(("stepper", i)), Sense::click());
        let h = hover_t(ui, &resp);
        ui.painter().rect_filled(
            r.shrink(2.0),
            CornerRadius::same(5),
            mix(Color32::TRANSPARENT, t::HOVER, h),
        );
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(15.0),
            mix(t::TEXT_2, t::TEXT, h),
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *value = (*value + delta).clamp(*range.start(), *range.end());
            changed = true;
        }
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{}{unit}", *value as i32),
        FontId::proportional(12.5),
        t::TEXT,
    );
    changed
}

/// Colour for an account avatar, by the account's position in the list.
pub fn avatar_color(index: usize) -> Color32 {
    const PALETTE: [Color32; 6] = [
        Color32::from_rgb(0xc9, 0x6a, 0x4a),
        Color32::from_rgb(0x4c, 0x8d, 0xf6),
        Color32::from_rgb(0x3f, 0xa8, 0x6b),
        Color32::from_rgb(0x9b, 0x6c, 0xd8),
        Color32::from_rgb(0x2f, 0xa9, 0xb3),
        Color32::from_rgb(0xc8, 0x8a, 0x2a),
    ];
    PALETTE[index % PALETTE.len()]
}

/// Account group header in the session list: colour dot, name, count;
/// "In use" on the active account, "Switch" on hover for the others.
pub fn account_header(
    ui: &mut Ui,
    label: &str,
    count: usize,
    index: usize,
    in_use: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let h = if in_use { 0.0 } else { hover_t(ui, &resp) };
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        mix(Color32::TRANSPARENT, t::HOVER, h),
    );
    let c = avatar_color(index);
    ui.painter()
        .circle_filled(pos2(rect.min.x + 14.0, rect.center().y), 4.0, c);
    let name = ui.painter().text(
        pos2(rect.min.x + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.5),
        mix(t::TEXT_2, t::TEXT, h),
    );
    ui.painter().text(
        pos2(name.max.x + 7.0, rect.center().y),
        Align2::LEFT_CENTER,
        count.to_string(),
        FontId::proportional(11.5),
        t::TEXT_3,
    );
    let right = pos2(rect.max.x - 10.0, rect.center().y);
    if in_use {
        ui.painter().text(
            right,
            Align2::RIGHT_CENTER,
            "In use",
            FontId::proportional(11.0),
            c,
        );
    } else if h > 0.01 {
        ui.painter().text(
            right,
            Align2::RIGHT_CENTER,
            "Switch",
            FontId::proportional(11.0),
            t::TEXT_2.gamma_multiply(h),
        );
    }
    if in_use {
        resp
    } else {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// Round initial avatar.
pub fn paint_avatar(ui: &Ui, center: egui::Pos2, radius: f32, initial: char, index: usize) {
    let c = avatar_color(index);
    ui.painter().circle_filled(center, radius, c);
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(radius * 1.05),
        Color32::WHITE,
    );
}

/// Round send button (arrow up), accent when enabled.
pub fn send_button(ui: &mut Ui, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), sense);
    let h = hover_t(ui, &resp);
    let fill = if enabled {
        mix(t::ACCENT, t::ACCENT_HOVER, h)
    } else {
        t::BG_ELEVATED_2
    };
    ui.painter().circle_filled(rect.center(), 14.0, fill);
    let fg = if enabled { Color32::WHITE } else { t::TEXT_3 };
    paint_icon(ui, rect.shrink(7.5), Icon::Send, fg);
    let resp = resp.on_hover_text("Send (Enter)");
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
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
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let h = hover_t(ui, &resp);
    let painter = ui.painter_at(rect);
    let bg = if spec.active {
        t::ACTIVE
    } else {
        mix(Color32::TRANSPARENT, t::HOVER, h)
    };
    painter.rect_filled(rect, CornerRadius::same(6), bg);
    if let Some(a) = spec.accent {
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.min.x, rect.min.y + 10.0),
                vec2(2.0, rect.height() - 20.0),
            ),
            CornerRadius::same(2),
            a,
        );
    }
    let left = rect.min.x + 10.0;
    let icon_c = pos2(left + 7.0, rect.min.y + 13.5);
    match spec.icon {
        Some(i) => paint_icon(
            ui,
            Rect::from_center_size(icon_c, vec2(14.0, 14.0)),
            i,
            if spec.active {
                t::TEXT_1
            } else {
                mix(t::TEXT_3, t::TEXT_2, h)
            },
        ),
        None => {
            painter.circle_filled(icon_c, 3.5, spec.dot);
        }
    }
    let text_x = left + 24.0;
    let trailing_w = spec
        .trailing
        .map(|s| text_w(ui, s, FontId::proportional(11.0)) + 10.0)
        .unwrap_or(0.0);
    let max_w = (rect.max.x - text_x - 10.0 - trailing_w).max(20.0);
    let title_col = if spec.active { t::TEXT } else { t::TEXT_1 };
    let title = elide(ui, spec.title, FontId::proportional(12.5), title_col, max_w);
    painter.galley(pos2(text_x, rect.min.y + 5.0), title, title_col);
    // Status dot leads the subtitle ("● Waiting for you · ~/app").
    let sub_x = if spec.icon.is_some() {
        painter.circle_filled(pos2(text_x + 2.5, rect.min.y + 28.5), 2.5, spec.dot);
        text_x + 10.0
    } else {
        text_x
    };
    let sub = elide(
        ui,
        spec.subtitle,
        FontId::proportional(11.0),
        t::TEXT_3,
        max_w + trailing_w - (sub_x - text_x),
    );
    painter.galley(pos2(sub_x, rect.min.y + 22.0), sub, t::TEXT_3);
    if let Some(tr) = spec.trailing {
        painter.text(
            pos2(rect.max.x - 10.0, rect.min.y + 13.0),
            Align2::RIGHT_CENTER,
            tr,
            FontId::proportional(11.0),
            t::TEXT_3,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
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
        .stroke(Stroke::new(1.0, t::BORDER_STRONG))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(12))
        .shadow(egui::Shadow {
            offset: [0, 18],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(160),
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
