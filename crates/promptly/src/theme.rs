//! Colors. Restrained chrome; color is reserved for session state
//! (amber waiting, blue working, green done, red error).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;
use promptly_core::state::SessionState;

pub const BG: Rgb = Rgb {
    r: 0x13,
    g: 0x14,
    b: 0x17,
};
pub const FG: Rgb = Rgb {
    r: 0xd8,
    g: 0xd9,
    b: 0xdc,
};
pub const CURSOR: Color32 = Color32::from_rgb(0xd8, 0xd9, 0xdc);
pub const SELECTION: Color32 = Color32::from_rgb(0x33, 0x42, 0x5c);

pub const AMBER: Color32 = Color32::from_rgb(0xe0, 0xa5, 0x2e);
pub const BLUE: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xf6);
pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
pub const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x8e, 0x96);

/// Tango-ish base 16, readable on the dark background (>= 4.5:1 for the
/// bright set and the foreground).
const ANSI: [Rgb; 16] = [
    Rgb {
        r: 0x2e,
        g: 0x30,
        b: 0x36,
    },
    Rgb {
        r: 0xe5,
        g: 0x53,
        b: 0x4b,
    },
    Rgb {
        r: 0x3f,
        g: 0xb9,
        b: 0x50,
    },
    Rgb {
        r: 0xd2,
        g: 0x99,
        b: 0x22,
    },
    Rgb {
        r: 0x4c,
        g: 0x8d,
        b: 0xf6,
    },
    Rgb {
        r: 0xb0,
        g: 0x7c,
        b: 0xe8,
    },
    Rgb {
        r: 0x39,
        g: 0xc5,
        b: 0xcf,
    },
    Rgb {
        r: 0xc9,
        g: 0xcc,
        b: 0xd1,
    },
    Rgb {
        r: 0x6e,
        g: 0x76,
        b: 0x81,
    },
    Rgb {
        r: 0xff,
        g: 0x7b,
        b: 0x72,
    },
    Rgb {
        r: 0x56,
        g: 0xd3,
        b: 0x64,
    },
    Rgb {
        r: 0xe3,
        g: 0xb3,
        b: 0x41,
    },
    Rgb {
        r: 0x79,
        g: 0xc0,
        b: 0xff,
    },
    Rgb {
        r: 0xd2,
        g: 0xa8,
        b: 0xff,
    },
    Rgb {
        r: 0x56,
        g: 0xd4,
        b: 0xdd,
    },
    Rgb {
        r: 0xf0,
        g: 0xf6,
        b: 0xfc,
    },
];

/// xterm 256-color palette plus named slots.
pub fn color_for_index(idx: usize) -> Rgb {
    match idx {
        0..=15 => ANSI[idx],
        16..=231 => {
            let i = idx - 16;
            let step = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            Rgb {
                r: step(i / 36),
                g: step((i / 6) % 6),
                b: step(i % 6),
            }
        }
        232..=255 => {
            let v = (8 + (idx - 232) * 10) as u8;
            Rgb { r: v, g: v, b: v }
        }
        i if i == NamedColor::Foreground as usize || i == NamedColor::BrightForeground as usize => {
            FG
        }
        i if i == NamedColor::Background as usize => BG,
        i if i == NamedColor::Cursor as usize => FG,
        i if i == NamedColor::DimForeground as usize => dim(FG),
        i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
            dim(ANSI[i - NamedColor::DimBlack as usize])
        }
        _ => FG,
    }
}

fn dim(c: Rgb) -> Rgb {
    Rgb {
        r: (c.r as u16 * 2 / 3) as u8,
        g: (c.g as u16 * 2 / 3) as u8,
        b: (c.b as u16 * 2 / 3) as u8,
    }
}

/// Resolve a cell color, honoring OSC 4/10/11 overrides set by the program.
pub fn resolve(c: Color, overrides: &Colors) -> Rgb {
    match c {
        Color::Spec(rgb) => rgb,
        Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| color_for_index(i as usize)),
        Color::Named(n) => overrides[n].unwrap_or_else(|| color_for_index(n as usize)),
    }
}

pub fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

pub fn state_color(s: SessionState) -> Color32 {
    match s {
        SessionState::WaitingPermission | SessionState::WaitingInput => AMBER,
        SessionState::Working | SessionState::Starting => BLUE,
        SessionState::Done => GREEN,
        SessionState::Error => RED,
        SessionState::Idle | SessionState::Exited => MUTED,
    }
}

/// Design tokens. Surfaces step up in lightness with elevation; text has
/// three levels; the accent is reserved for the primary action and focus.
pub mod tokens {
    use egui::Color32;
    pub const BG_MAIN: Color32 = Color32::from_rgb(0x13, 0x14, 0x17);
    pub const BG_SIDEBAR: Color32 = Color32::from_rgb(0x19, 0x1a, 0x1e);
    pub const BG_ELEVATED: Color32 = Color32::from_rgb(0x1f, 0x20, 0x25);
    pub const BG_ELEVATED_2: Color32 = Color32::from_rgb(0x27, 0x28, 0x2e);
    pub const BG_INPUT: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x20);
    pub const HOVER: Color32 = Color32::from_rgb(0x22, 0x23, 0x28);
    pub const ACTIVE: Color32 = Color32::from_rgb(0x2a, 0x2b, 0x32);
    pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2b, 0x31);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x3b, 0x43);
    pub const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf4);
    pub const TEXT_1: Color32 = Color32::from_rgb(0xd4, 0xd5, 0xd9);
    pub const TEXT_2: Color32 = Color32::from_rgb(0xa3, 0xa6, 0xad);
    pub const TEXT_3: Color32 = Color32::from_rgb(0x74, 0x77, 0x7f);
    pub const ACCENT: Color32 = Color32::from_rgb(0xc9, 0x6a, 0x4a);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xd7, 0x78, 0x57);
    pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0xb5, 0x5c, 0x3e);
}

pub fn apply_chrome(ctx: &egui::Context, high_contrast: bool) {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};
    use tokens as t;
    let mut v = egui::Visuals::dark();
    v.panel_fill = t::BG_MAIN;
    v.window_fill = t::BG_ELEVATED;
    v.faint_bg_color = t::BG_SIDEBAR;
    v.extreme_bg_color = t::BG_INPUT;
    v.code_bg_color = t::BG_ELEVATED_2;
    v.hyperlink_color = Color32::from_rgb(0x8a, 0xb4, 0xf8);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, t::BORDER);
    v.window_shadow = egui::Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.selection.bg_fill = t::ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, t::ACCENT_HOVER);
    v.text_cursor.stroke = Stroke::new(2.0, t::ACCENT_HOVER);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t::TEXT_1);
    w.noninteractive.bg_fill = t::BG_MAIN;
    for (state, fill, stroke, fg) in [
        (&mut w.inactive, t::BG_ELEVATED_2, t::BORDER, t::TEXT_1),
        (&mut w.hovered, t::ACTIVE, t::BORDER_STRONG, t::TEXT),
        (&mut w.active, t::BORDER_STRONG, t::BORDER_STRONG, t::TEXT),
        (&mut w.open, t::ACTIVE, t::BORDER_STRONG, t::TEXT),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, fg);
        state.corner_radius = CornerRadius::same(6);
        state.expansion = 0.0;
    }
    if high_contrast {
        v.override_text_color = Some(Color32::WHITE);
        v.widgets.noninteractive.bg_stroke.color = Color32::from_gray(0x80);
    }
    // Promptly is dark-only for now: don't let the OS light theme swap visuals.
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(17.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.window_margin = egui::Margin::same(16);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.animation_time = 0.0; // no decorative motion
        s.interaction.selectable_labels = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Rgb) -> f64 {
        let f = |v: u8| {
            let s = v as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    #[test]
    fn foreground_and_bright_colors_meet_4_5_to_1() {
        let bg = lum(BG);
        for c in std::iter::once(FG).chain(ANSI[9..].iter().copied()) {
            let ratio = (lum(c) + 0.05) / (bg + 0.05);
            assert!(ratio >= 4.5, "{c:?} has contrast {ratio:.2}");
        }
    }

    #[test]
    fn cube_and_grays() {
        assert_eq!(color_for_index(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            color_for_index(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(color_for_index(232), Rgb { r: 8, g: 8, b: 8 });
    }
}
