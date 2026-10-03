//! Font setup: the chosen terminal font (Fira Code Retina by default) first,
//! then fallbacks for symbols and emoji (Claude Code's TUI uses ⏺ ✻ ⎿ and
//! box drawing).

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

/// The built-in terminal font (SIL OFL, see assets/FIRACODE-LICENSE.txt).
pub const BUILTIN: &str = "Fira Code Retina";
const FIRA_RETINA: &[u8] = include_bytes!("../assets/FiraCode-Retina.ttf");
const FIRA_BOLD: &[u8] = include_bytes!("../assets/FiraCode-Bold.ttf");

/// Font family for bold terminal text.
pub fn bold_family() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

/// One installed monospace family: its regular and bold faces.
#[derive(Clone, Debug)]
pub struct MonoFamily {
    pub name: String,
    regular: (std::path::PathBuf, u32),
    bold: Option<(std::path::PathBuf, u32)>,
}

/// Monospace families installed on this machine, by name (a few hundred
/// milliseconds: call it off the UI thread).
pub fn system_monospace() -> Vec<MonoFamily> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut by_name: std::collections::BTreeMap<String, MonoFamily> = Default::default();
    for f in db.faces() {
        if !f.monospaced || f.style != fontdb::Style::Normal {
            continue;
        }
        let fontdb::Source::File(path) = &f.source else {
            continue;
        };
        let Some((name, _)) = f.families.first() else {
            continue;
        };
        if name.starts_with('.') {
            continue; // hidden system faces
        }
        let face = (path.clone(), f.index);
        let e = by_name.entry(name.clone()).or_insert_with(|| MonoFamily {
            name: name.clone(),
            regular: face.clone(),
            bold: None,
        });
        // Prefer the face closest to regular (400) and to bold (700).
        let w = f.weight.0;
        if w == 400 {
            e.regular = face.clone();
        }
        if w == 700 || (w > 400 && e.bold.is_none()) {
            e.bold = Some(face);
        }
    }
    by_name.into_values().collect()
}

/// Regular and bold font data for a terminal family; the built-in font when
/// it's the default or isn't installed.
fn terminal_faces(family: &str) -> (FontData, Option<FontData>) {
    let load = |(p, i): &(std::path::PathBuf, u32)| {
        std::fs::read(p).ok().map(|b| {
            let mut fd = FontData::from_owned(b);
            fd.index = *i;
            fd
        })
    };
    if !family.is_empty()
        && family != BUILTIN
        && let Some(f) = system_monospace().into_iter().find(|f| f.name == family)
        && let Some(regular) = load(&f.regular)
    {
        return (regular, f.bold.as_ref().and_then(load));
    }
    (
        FontData::from_static(FIRA_RETINA),
        Some(FontData::from_static(FIRA_BOLD)),
    )
}

/// Install fonts with `family` as the terminal font (see [`BUILTIN`]).
/// `PROMPTLY_FONT` (a font file path) still overrides it.
pub fn install(ctx: &egui::Context, family: &str) {
    let mut fonts = FontDefinitions::default();
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    let builtin: Vec<String> = mono.clone();
    let mut chain = Vec::new();
    let custom = std::env::var("PROMPTLY_FONT")
        .ok()
        .and_then(|p| std::fs::read(p).ok());
    let (regular, bold) = match custom {
        Some(bytes) => (FontData::from_owned(bytes), None),
        None => terminal_faces(family),
    };
    fonts.font_data.insert("primary".into(), Arc::new(regular));
    chain.push("primary".to_string());
    // The system face stays available for glyphs the chosen font lacks.
    for (path, index) in PRIMARY.iter().copied() {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fd = FontData::from_owned(bytes);
            fd.index = index;
            fonts.font_data.insert("system-mono".into(), Arc::new(fd));
            chain.push("system-mono".to_string());
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
    // Bold terminal text: the bold face, then the regular chain.
    let mut bold_chain = chain.clone();
    if let Some(b) = bold {
        fonts.font_data.insert("primary-bold".into(), Arc::new(b));
        bold_chain.insert(0, "primary-bold".into());
    }
    fonts.families.insert(bold_family(), bold_chain);
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
