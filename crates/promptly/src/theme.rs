//! Colors and themes. Restrained chrome; color is reserved for session
//! state (amber waiting, blue working, green done, red error).
//!
//! Themes mimic the most-installed editor themes (VS Code Marketplace:
//! GitHub Theme, One Dark Pro, Dracula, Tokyo Night, Catppuccin, Nord,
//! Night Owl, Monokai Pro), each with its own terminal palette. Colors are
//! read through functions (`tokens::TEXT()`, `AMBER()`) so the whole app
//! follows the theme picked in Settings without a restart.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;
use promptly_core::state::SessionState;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicPtr, Ordering};

/// What a theme defines; everything else is derived the same way for all.
struct Spec {
    name: &'static str,
    light: bool,
    /// Editor / terminal background.
    bg: u32,
    sidebar: u32,
    /// Panels, cards, popups.
    panel: u32,
    border: u32,
    text: u32,
    /// Secondary text (comments, captions).
    muted: u32,
    accent: u32,
    selection: u32,
    /// Terminal foreground and the 16 ANSI colors.
    fg: u32,
    ansi: [u32; 16],
}

const SPECS: &[Spec] = &[
    Spec {
        name: "Promptly",
        light: false,
        bg: 0x0f1012,
        sidebar: 0x141518,
        panel: 0x191a1e,
        border: 0x232429,
        text: 0xeeeef0,
        muted: 0x8e919a,
        accent: 0xd97757,
        selection: 0x33425c,
        fg: 0xd8d9dc,
        ansi: [
            0x2e3036, 0xe5534b, 0x3fb950, 0xd29922, 0x4c8df6, 0xb07ce8, 0x39c5cf, 0xc9ccd1,
            0x6e7681, 0xff7b72, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xf0f6fc,
        ],
    },
    Spec {
        name: "GitHub Dark",
        light: false,
        bg: 0x0d1117,
        sidebar: 0x010409,
        panel: 0x161b22,
        border: 0x30363d,
        text: 0xe6edf3,
        muted: 0x8d96a0,
        accent: 0x2f81f7,
        selection: 0x264f78,
        fg: 0xe6edf3,
        ansi: [
            0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
            0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
        ],
    },
    Spec {
        name: "GitHub Light",
        light: true,
        bg: 0xffffff,
        sidebar: 0xf6f8fa,
        panel: 0xf6f8fa,
        border: 0xd0d7de,
        text: 0x1f2328,
        muted: 0x656d76,
        accent: 0x0969da,
        selection: 0xb6e3ff,
        fg: 0x1f2328,
        ansi: [
            0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
            0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
        ],
    },
    Spec {
        name: "One Dark Pro",
        light: false,
        bg: 0x282c34,
        sidebar: 0x21252b,
        panel: 0x2c313a,
        border: 0x3e4452,
        text: 0xd7dae0,
        muted: 0x7f848e,
        accent: 0x61afef,
        selection: 0x3e4451,
        fg: 0xabb2bf,
        ansi: [
            0x3f4451, 0xe05561, 0x8cc265, 0xd18f52, 0x4aa5f0, 0xc162de, 0x42b3c2, 0xd7dae0,
            0x4f5666, 0xff616e, 0xa5e075, 0xf0a45d, 0x4dc4ff, 0xde73ff, 0x4cd1e0, 0xe6e6e6,
        ],
    },
    Spec {
        name: "Dracula",
        light: false,
        bg: 0x282a36,
        sidebar: 0x21222c,
        panel: 0x343746,
        border: 0x44475a,
        text: 0xf8f8f2,
        muted: 0x6272a4,
        accent: 0xbd93f9,
        selection: 0x44475a,
        fg: 0xf8f8f2,
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    },
    Spec {
        name: "Tokyo Night",
        light: false,
        bg: 0x1a1b26,
        sidebar: 0x16161e,
        panel: 0x1f2335,
        border: 0x29355a,
        text: 0xc0caf5,
        muted: 0x787c99,
        accent: 0x7aa2f7,
        selection: 0x283457,
        fg: 0xa9b1d6,
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    Spec {
        name: "Catppuccin Mocha",
        light: false,
        bg: 0x1e1e2e,
        sidebar: 0x181825,
        panel: 0x313244,
        border: 0x45475a,
        text: 0xcdd6f4,
        muted: 0x9399b2,
        accent: 0xcba6f7,
        selection: 0x45475a,
        fg: 0xcdd6f4,
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    },
    Spec {
        name: "Nord",
        light: false,
        bg: 0x2e3440,
        sidebar: 0x2b303b,
        panel: 0x3b4252,
        border: 0x434c5e,
        text: 0xeceff4,
        muted: 0x8d96a8,
        accent: 0x88c0d0,
        selection: 0x434c5e,
        fg: 0xd8dee9,
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    Spec {
        name: "Night Owl",
        light: false,
        bg: 0x011627,
        sidebar: 0x011627,
        panel: 0x0b2942,
        border: 0x122d42,
        text: 0xd6deeb,
        muted: 0x637777,
        accent: 0x82aaff,
        selection: 0x1d3b53,
        fg: 0xd6deeb,
        ansi: [
            0x011627, 0xef5350, 0x22da6e, 0xaddb67, 0x82aaff, 0xc792ea, 0x21c7a8, 0xffffff,
            0x575656, 0xef5350, 0x22da6e, 0xffeb95, 0x82aaff, 0xc792ea, 0x7fdbca, 0xffffff,
        ],
    },
    Spec {
        name: "Monokai Pro",
        light: false,
        bg: 0x2d2a2e,
        sidebar: 0x221f22,
        panel: 0x363337,
        border: 0x403e41,
        text: 0xfcfcfa,
        muted: 0x939293,
        accent: 0xffd866,
        selection: 0x5b595c,
        fg: 0xfcfcfa,
        ansi: [
            0x403e41, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
            0x727072, 0xff6188, 0xa9dc76, 0xffd866, 0xfc9867, 0xab9df2, 0x78dce8, 0xfcfcfa,
        ],
    },
];

/// A full set of colors, derived once from a [`Spec`].
pub struct Palette {
    pub name: &'static str,
    /// Imported by the user (can be removed).
    pub custom: bool,
    pub light: bool,
    pub bg_main: Color32,
    pub bg_sidebar: Color32,
    pub bg_elevated: Color32,
    pub bg_elevated_2: Color32,
    pub bg_input: Color32,
    pub hover: Color32,
    pub active: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_1: Color32,
    pub text_2: Color32,
    pub text_3: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub amber: Color32,
    pub blue: Color32,
    pub green: Color32,
    pub red: Color32,
    pub muted: Color32,
    pub term_bg: Rgb,
    pub term_fg: Rgb,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Rgb; 16],
}

fn rgb(hex: u32) -> Rgb {
    Rgb {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
    }
}

fn col(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// a + (b - a) * k, per channel.
fn blend(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl Palette {
    fn from_spec(s: &Spec) -> Self {
        let (bg, panel, text, muted, accent) = (
            col(s.bg),
            col(s.panel),
            col(s.text),
            col(s.muted),
            col(s.accent),
        );
        let ink = if s.light {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        let ansi = s.ansi.map(rgb);
        let c = |i: usize| col(s.ansi[i]);
        Self {
            name: s.name,
            custom: false,
            light: s.light,
            bg_main: bg,
            bg_sidebar: col(s.sidebar),
            bg_elevated: panel,
            bg_elevated_2: blend(panel, text, 0.06),
            bg_input: blend(bg, panel, 0.5),
            hover: blend(panel, text, 0.04),
            active: blend(panel, text, 0.09),
            border: col(s.border),
            border_strong: blend(col(s.border), text, 0.12),
            text,
            text_1: blend(text, bg, 0.12),
            text_2: muted,
            text_3: blend(muted, bg, 0.3),
            accent,
            accent_hover: blend(accent, ink, 0.12),
            accent_pressed: blend(accent, bg, 0.15),
            // Session states follow the theme's own terminal colors.
            amber: if s.name == "Promptly" {
                col(0xe5a842)
            } else {
                c(3)
            },
            blue: if s.name == "Promptly" {
                col(0x5b9cf5)
            } else {
                c(4)
            },
            green: if s.name == "Promptly" {
                col(0x4cc38a)
            } else {
                c(2)
            },
            red: if s.name == "Promptly" {
                col(0xec6a5e)
            } else {
                c(1)
            },
            muted: blend(muted, bg, 0.15),
            term_bg: rgb(s.bg),
            term_fg: rgb(s.fg),
            cursor: col(s.fg),
            selection: col(s.selection),
            ansi,
        }
    }
}

/// Built-in themes, then imported ones. Palettes live for the whole run
/// (imports are few and small), so lookups need no lock.
static REGISTRY: LazyLock<parking_lot::RwLock<Vec<&'static Palette>>> = LazyLock::new(|| {
    parking_lot::RwLock::new(
        SPECS
            .iter()
            .map(|s| &*Box::leak(Box::new(Palette::from_spec(s))))
            .collect(),
    )
});
static CURRENT: AtomicPtr<Palette> = AtomicPtr::new(std::ptr::null_mut());

/// The active palette.
pub fn current() -> &'static Palette {
    let p = CURRENT.load(Ordering::Relaxed);
    if p.is_null() {
        REGISTRY.read()[0]
    } else {
        // SAFETY: only ever set to a leaked, never-freed &'static Palette.
        unsafe { &*p }
    }
}

/// Every theme, in menu order.
pub fn all() -> Vec<&'static Palette> {
    REGISTRY.read().clone()
}

/// Switch themes by name (unknown names fall back to the default). Call
/// [`apply_chrome`] afterwards so egui's own widgets follow.
pub fn set(name: &str) {
    let reg = REGISTRY.read();
    let p = reg
        .iter()
        .find(|p| p.name == name)
        .copied()
        .unwrap_or(reg[0]);
    CURRENT.store(p as *const Palette as *mut Palette, Ordering::Relaxed);
}

/// Imported themes are kept here, one VS Code `.json` per theme.
fn custom_dir() -> std::path::PathBuf {
    promptly_core::paths::data_dir().join("themes")
}

fn register(t: promptly_core::vscode_theme::Imported) -> &'static Palette {
    let spec = Spec {
        name: Box::leak(t.name.clone().into_boxed_str()),
        light: t.light,
        bg: t.bg,
        sidebar: t.sidebar,
        panel: t.panel,
        border: t.border,
        text: t.text,
        muted: t.muted,
        accent: t.accent,
        selection: t.selection,
        fg: t.fg,
        ansi: t.ansi,
    };
    let mut p = Palette::from_spec(&spec);
    p.custom = true;
    let p: &'static Palette = Box::leak(Box::new(p));
    let mut reg = REGISTRY.write();
    reg.retain(|x| !(x.custom && x.name == p.name));
    reg.push(p);
    p
}

/// Load the themes imported earlier (call at startup, before [`set`]).
pub fn load_custom() {
    for e in std::fs::read_dir(custom_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(src) = std::fs::read_to_string(e.path())
            && let Ok(t) = promptly_core::vscode_theme::parse(&src)
        {
            register(t);
        }
    }
}

/// Import a VS Code theme file; keeps a copy and returns the theme's name.
pub fn import(path: &std::path::Path) -> Result<String, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let t = promptly_core::vscode_theme::parse(&src)?;
    if SPECS.iter().any(|s| s.name == t.name) {
        return Err(format!("\u{201c}{}\u{201d} is already built in", t.name));
    }
    let dir = custom_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(file_for(&t.name)), &src).map_err(|e| e.to_string())?;
    Ok(register(t).name.to_string())
}

/// Remove an imported theme (built-in ones can't be removed).
pub fn remove(name: &str) {
    let mut reg = REGISTRY.write();
    reg.retain(|p| !(p.custom && p.name == name));
    let _ = std::fs::remove_file(custom_dir().join(file_for(name)));
}

fn file_for(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("{slug}.json")
}

pub const DEFAULT: &str = "Promptly";

#[allow(non_snake_case)]
pub fn BG() -> Rgb {
    current().term_bg
}
#[allow(non_snake_case)]
pub fn CURSOR() -> Color32 {
    current().cursor
}
#[allow(non_snake_case)]
pub fn SELECTION() -> Color32 {
    current().selection
}
#[allow(non_snake_case)]
pub fn AMBER() -> Color32 {
    current().amber
}
#[allow(non_snake_case)]
pub fn BLUE() -> Color32 {
    current().blue
}
#[allow(non_snake_case)]
pub fn GREEN() -> Color32 {
    current().green
}
#[allow(non_snake_case)]
pub fn RED() -> Color32 {
    current().red
}
#[allow(non_snake_case)]
pub fn MUTED() -> Color32 {
    current().muted
}

/// xterm 256-color palette plus named slots.
pub fn color_for_index(idx: usize) -> Rgb {
    let p = current();
    match idx {
        0..=15 => p.ansi[idx],
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
            p.term_fg
        }
        i if i == NamedColor::Background as usize => p.term_bg,
        i if i == NamedColor::Cursor as usize => p.term_fg,
        i if i == NamedColor::DimForeground as usize => dim(p.term_fg),
        i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
            dim(p.ansi[i - NamedColor::DimBlack as usize])
        }
        _ => p.term_fg,
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
        SessionState::WaitingPermission | SessionState::WaitingInput => AMBER(),
        SessionState::Working | SessionState::Starting => BLUE(),
        SessionState::Done => GREEN(),
        SessionState::Error => RED(),
        SessionState::Idle | SessionState::Exited => MUTED(),
    }
}

/// Design tokens of the active theme. Surfaces step up with elevation; text
/// has three levels; the accent is reserved for the primary action and focus.
#[allow(non_snake_case)]
pub mod tokens {
    use super::current;
    use egui::Color32;
    pub fn BG_MAIN() -> Color32 {
        current().bg_main
    }
    pub fn BG_SIDEBAR() -> Color32 {
        current().bg_sidebar
    }
    pub fn BG_ELEVATED() -> Color32 {
        current().bg_elevated
    }
    pub fn BG_ELEVATED_2() -> Color32 {
        current().bg_elevated_2
    }
    pub fn BG_INPUT() -> Color32 {
        current().bg_input
    }
    pub fn HOVER() -> Color32 {
        current().hover
    }
    pub fn ACTIVE() -> Color32 {
        current().active
    }
    pub fn BORDER() -> Color32 {
        current().border
    }
    pub fn BORDER_STRONG() -> Color32 {
        current().border_strong
    }
    pub fn TEXT() -> Color32 {
        current().text
    }
    pub fn TEXT_1() -> Color32 {
        current().text_1
    }
    pub fn TEXT_2() -> Color32 {
        current().text_2
    }
    pub fn TEXT_3() -> Color32 {
        current().text_3
    }
    pub fn ACCENT() -> Color32 {
        current().accent
    }
    pub fn ACCENT_HOVER() -> Color32 {
        current().accent_hover
    }
    pub fn ACCENT_PRESSED() -> Color32 {
        current().accent_pressed
    }
}

/// Text color that reads on top of the accent (dark text on light accents
/// such as Monokai's yellow or Nord's frost).
pub fn on_accent() -> Color32 {
    let a = tokens::ACCENT();
    let l = 0.2126 * a.r() as f32 + 0.7152 * a.g() as f32 + 0.0722 * a.b() as f32;
    if l > 160.0 {
        Color32::from_rgb(0x14, 0x14, 0x16)
    } else {
        Color32::WHITE
    }
}

pub fn apply_chrome(ctx: &egui::Context, high_contrast: bool) {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};
    use tokens as t;
    let light = current().light;
    let mut v = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    v.panel_fill = t::BG_MAIN();
    v.window_fill = t::BG_ELEVATED();
    v.faint_bg_color = t::BG_SIDEBAR();
    v.extreme_bg_color = t::BG_INPUT();
    v.code_bg_color = t::BG_ELEVATED_2();
    v.hyperlink_color = BLUE();
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, t::BORDER());
    let shadow: u8 = if light { 40 } else { 140 };
    v.window_shadow = egui::Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha((shadow as u16 * 6 / 7) as u8),
    };
    v.selection.bg_fill = t::ACCENT().gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, t::ACCENT_HOVER());
    v.text_cursor.stroke = Stroke::new(2.0, t::ACCENT_HOVER());
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t::BORDER());
    w.noninteractive.fg_stroke = Stroke::new(1.0, t::TEXT_1());
    w.noninteractive.bg_fill = t::BG_MAIN();
    for (state, fill, stroke, fg) in [
        (
            &mut w.inactive,
            t::BG_ELEVATED_2(),
            t::BORDER(),
            t::TEXT_1(),
        ),
        (&mut w.hovered, t::ACTIVE(), t::BORDER_STRONG(), t::TEXT()),
        (
            &mut w.active,
            t::BORDER_STRONG(),
            t::BORDER_STRONG(),
            t::TEXT(),
        ),
        (&mut w.open, t::ACTIVE(), t::BORDER_STRONG(), t::TEXT()),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, fg);
        state.corner_radius = CornerRadius::same(6);
        state.expansion = 0.0;
    }
    if high_contrast {
        v.override_text_color = Some(if light {
            Color32::BLACK
        } else {
            Color32::WHITE
        });
        v.widgets.noninteractive.bg_stroke.color = Color32::from_gray(0x80);
    }
    // The theme decides light or dark, not the OS setting.
    let which = if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    ctx.set_theme(which);
    ctx.set_visuals_of(which, v);
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
        s.interaction.tooltip_delay = 0.35;
        // Thin scrollbars that float over content, as on macOS.
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.tooltip_width = 320.0;
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

    fn ratio(a: Rgb, b: Rgb) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    fn rgb32(c: Color32) -> Rgb {
        Rgb {
            r: c.r(),
            g: c.g(),
            b: c.b(),
        }
    }

    #[test]
    fn default_foreground_and_bright_colors_meet_4_5_to_1() {
        let p = &all()[0];
        assert_eq!(p.name, DEFAULT);
        for c in std::iter::once(p.term_fg).chain(p.ansi[9..].iter().copied()) {
            let r = ratio(c, p.term_bg);
            assert!(r >= 4.5, "{c:?} has contrast {r:.2}");
        }
    }

    #[test]
    fn every_theme_keeps_text_readable() {
        for p in all() {
            // Body text and terminal text meet WCAG AA on their backgrounds.
            assert!(
                ratio(p.term_fg, p.term_bg) >= 4.5,
                "{}: terminal text",
                p.name
            );
            assert!(
                ratio(rgb32(p.text), rgb32(p.bg_main)) >= 7.0,
                "{}: interface text",
                p.name
            );
            assert!(
                ratio(rgb32(p.text_2), rgb32(p.bg_main)) >= 3.0,
                "{}: secondary text",
                p.name
            );
            assert_eq!(p.light, lum(p.term_bg) > 0.5, "{}: light flag", p.name);
        }
    }

    #[test]
    fn import_registers_and_removes() {
        let d = tempfile::tempdir().unwrap();
        // SAFETY: test-only; nothing else reads PROMPTLY_DATA_DIR here.
        unsafe { std::env::set_var("PROMPTLY_DATA_DIR", d.path()) };
        let f = d.path().join("ayu.json");
        std::fs::write(
            &f,
            r##"{"name":"Test Ayu","type":"dark","colors":{"editor.background":"#0b0e14","editor.foreground":"#bfbdb6","button.background":"#e6b450"}}"##,
        )
        .unwrap();
        assert_eq!(import(&f).unwrap(), "Test Ayu");
        let p = all().into_iter().find(|p| p.name == "Test Ayu").unwrap();
        assert!(p.custom);
        assert_eq!(p.accent, Color32::from_rgb(0xe6, 0xb4, 0x50));
        assert!(
            d.path().join("themes/test-ayu.json").exists(),
            "a copy is kept"
        );
        remove("Test Ayu");
        assert!(all().iter().all(|p| p.name != "Test Ayu"));
        assert!(!d.path().join("themes/test-ayu.json").exists());
    }

    #[test]
    fn set_switches_and_falls_back() {
        set("Dracula");
        assert_eq!(current().name, "Dracula");
        assert_eq!(
            BG(),
            Rgb {
                r: 0x28,
                g: 0x2a,
                b: 0x36
            }
        );
        set("No such theme");
        assert_eq!(current().name, DEFAULT);
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
