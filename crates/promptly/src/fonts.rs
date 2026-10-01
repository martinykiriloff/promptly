//! Font setup: a system monospace face first, then fallbacks for symbols
//! and emoji (Claude Code's TUI uses ⏺ ✻ ⎿ and box drawing).

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

#[cfg(target_os = "macos")]
const PRIMARY: &[(&str, u32)] = &[
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
    ("/System/Library/Fonts/Menlo.ttc", 0),
];
#[cfg(target_os = "macos")]
const FALLBACK: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Apple Symbols.ttf", 0),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
];

#[cfg(target_os = "macos")]
const UI: &[&str] = &[
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/HelveticaNeue.ttc",
];
#[cfg(not(target_os = "macos"))]
const UI: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
    "/usr/share/fonts/google-inter/Inter-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-VF.otf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
];

#[cfg(not(target_os = "macos"))]
const PRIMARY: &[(&str, u32)] = &[
    (
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        0,
    ),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 0),
    (
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
        0,
    ),
    ("/usr/share/fonts/TTF/DejaVuSansMono.ttf", 0),
];
#[cfg(not(target_os = "macos"))]
const FALLBACK: &[(&str, u32)] = &[
    (
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        0,
    ),
    (
        "/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf",
        0,
    ),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
];

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    let builtin: Vec<String> = mono.clone();
    let mut chain = Vec::new();
    let custom = std::env::var("PROMPTLY_FONT").ok();
    let primary = custom
        .iter()
        .map(|p| (p.as_str(), 0))
        .chain(PRIMARY.iter().copied());
    for (path, index) in primary {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fd = FontData::from_owned(bytes);
            fd.index = index;
            fonts.font_data.insert("primary".into(), Arc::new(fd));
            chain.push("primary".to_string());
            break;
        }
    }
    // Built-in Hack + emoji keep working if no system font was found.
    chain.extend(builtin);
    for (i, (path, index)) in FALLBACK.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("fallback-{i}");
            let mut fd = FontData::from_owned(bytes);
            fd.index = *index;
            fonts.font_data.insert(name.clone(), Arc::new(fd));
            chain.push(name);
        }
    }
    // Interface font: the platform's system UI face ahead of egui's default.
    if let Some(bytes) = UI.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts
            .font_data
            .insert("ui".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui".into());
    }
    // UI text also needs the symbol fallbacks (⌘ ⌃ ↵ ⎇ in hints and headers).
    let fallbacks: Vec<String> = chain
        .iter()
        .filter(|n| n.starts_with("fallback-"))
        .cloned()
        .collect();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .extend(fallbacks);
    fonts.families.insert(FontFamily::Monospace, chain);
    // Lucide icon font (ISC licence, see assets/LUCIDE-LICENSE.txt).
    fonts.font_data.insert(
        "lucide".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/lucide.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Name(crate::ui_kit::ICON_FAMILY.into()),
        vec!["lucide".into()],
    );
    ctx.set_fonts(fonts);
}
