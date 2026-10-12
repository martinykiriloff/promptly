//! Import a VS Code color theme (`.json`, comments and trailing commas
//! allowed) into the colors a Promptly theme needs.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub name: String,
    pub light: bool,
    pub bg: u32,
    pub sidebar: u32,
    pub panel: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub selection: u32,
    pub fg: u32,
    pub ansi: [u32; 16],
}

/// Remove `//` and `/* */` comments (outside strings) and trailing commas.
fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: ",}" and ",]" (with whitespace between).
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            cleaned.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                cleaned.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == '}' || bytes[j] == ']') {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    cleaned
}

/// "#rgb", "#rrggbb" or "#rrggbbaa" (alpha blended over `under`).
fn hex(s: &str, under: u32) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    let (rgb, alpha) = match h.len() {
        3 => {
            let c: Vec<u32> = h.chars().filter_map(|c| c.to_digit(16)).collect();
            if c.len() != 3 {
                return None;
            }
            ((c[0] * 17) << 16 | (c[1] * 17) << 8 | (c[2] * 17), 255)
        }
        6 => (u32::from_str_radix(h, 16).ok()?, 255),
        8 => (
            u32::from_str_radix(&h[..6], 16).ok()?,
            u32::from_str_radix(&h[6..], 16).ok()?,
        ),
        _ => return None,
    };
    if alpha == 255 {
        return Some(rgb);
    }
    let mix = |sh: u32| {
        let a = (rgb >> sh) & 0xff;
        let b = (under >> sh) & 0xff;
        (a * alpha + b * (255 - alpha)) / 255
    };
    Some(mix(16) << 16 | mix(8) << 8 | mix(0))
}

fn luminance(c: u32) -> f32 {
    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

const DARK_ANSI: [u32; 16] = [
    0x2e3036, 0xe5534b, 0x3fb950, 0xd29922, 0x4c8df6, 0xb07ce8, 0x39c5cf, 0xc9ccd1, 0x6e7681,
    0xff7b72, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xf0f6fc,
];
const LIGHT_ANSI: [u32; 16] = [
    0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a,
    0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
];

pub fn parse(src: &str) -> Result<Imported, String> {
    let v: Value =
        serde_json::from_str(&strip_jsonc(src)).map_err(|e| format!("Not a theme file: {e}"))?;
    let colors = v
        .get("colors")
        .and_then(Value::as_object)
        .ok_or("This file has no \"colors\" (is it a VS Code color theme?)")?;
    let name = v["name"].as_str().unwrap_or("Imported theme").to_string();
    let bg = colors
        .get("editor.background")
        .and_then(Value::as_str)
        .and_then(|s| hex(s, 0))
        .ok_or("The theme has no editor.background")?;
    let pick = |keys: &[&str], under: u32| -> Option<u32> {
        keys.iter().find_map(|k| {
            colors
                .get(*k)
                .and_then(Value::as_str)
                .and_then(|s| hex(s, under))
        })
    };
    let light = match v["type"].as_str() {
        Some("light") | Some("hcLight") => true,
        Some(_) => false,
        None => luminance(bg) > 0.5,
    };
    let text = pick(&["editor.foreground", "foreground"], bg).unwrap_or(if light {
        0x1f2328
    } else {
        0xe6e6e6
    });
    let sidebar = pick(&["sideBar.background", "activityBar.background"], bg).unwrap_or(bg);
    let panel = pick(
        &[
            "editorWidget.background",
            "panel.background",
            "dropdown.background",
            "input.background",
        ],
        bg,
    )
    .unwrap_or(sidebar);
    let border = pick(
        &[
            "panel.border",
            "editorGroup.border",
            "sideBar.border",
            "widget.border",
            "focusBorder",
        ],
        bg,
    )
    .unwrap_or(panel);
    let muted = pick(
        &[
            "descriptionForeground",
            "editorLineNumber.foreground",
            "tab.inactiveForeground",
        ],
        bg,
    )
    .unwrap_or(text);
    let accent = pick(
        &[
            "button.background",
            "focusBorder",
            "textLink.foreground",
            "activityBarBadge.background",
        ],
        bg,
    )
    .unwrap_or(0x4c8df6);
    let selection = pick(
        &["editor.selectionBackground", "terminal.selectionBackground"],
        bg,
    )
    .unwrap_or(border);
    let fg = pick(&["terminal.foreground"], bg).unwrap_or(text);
    let names = [
        "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
    ];
    let defaults = if light { LIGHT_ANSI } else { DARK_ANSI };
    let mut ansi = defaults;
    for (i, n) in names.iter().enumerate() {
        if let Some(c) = pick(&[&format!("terminal.ansi{n}")], bg) {
            ansi[i] = c;
        }
        if let Some(c) = pick(&[&format!("terminal.ansiBright{n}")], bg) {
            ansi[i + 8] = c;
        }
    }
    Ok(Imported {
        name,
        light,
        bg,
        sidebar,
        panel,
        border,
        text,
        muted,
        accent,
        selection,
        fg,
        ansi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_jsonc_theme() {
        let src = r##"{
  // Ayu-ish
  "name": "My Ayu",
  "type": "dark",
  "colors": {
    "editor.background": "#0b0e14",
    "editor.foreground": "#bfbdb6", /* text */
    "sideBar.background": "#0d1017",
    "button.background": "#e6b450",
    "editor.selectionBackground": "#409fff4d",
    "terminal.ansiRed": "#ea6c73",
    "terminal.ansiBrightGreen": "#7fd962",
  },
  "tokenColors": [],
}"##;
        let t = parse(src).unwrap();
        assert_eq!(t.name, "My Ayu");
        assert!(!t.light);
        assert_eq!(
            (t.bg, t.text, t.sidebar, t.accent),
            (0x0b0e14, 0xbfbdb6, 0x0d1017, 0xe6b450)
        );
        assert_eq!(t.ansi[1], 0xea6c73);
        assert_eq!(t.ansi[10], 0x7fd962);
        assert_eq!(t.ansi[2], DARK_ANSI[2], "missing colors fall back");
        // 0x40 0x9f 0xff at alpha 0x4d over 0x0b0e14.
        assert_eq!(t.selection, hex("#409fff4d", 0x0b0e14).unwrap());
        assert_ne!(t.selection, 0x409fff);
    }

    #[test]
    fn light_from_luminance_and_errors() {
        let t = parse(r##"{"colors":{"editor.background":"#fafafa"}}"##).unwrap();
        assert!(t.light);
        assert_eq!(t.ansi, LIGHT_ANSI);
        assert!(parse("{}").is_err());
        assert!(parse("not json").is_err());
        assert_eq!(hex("#fff", 0), Some(0xffffff));
        assert_eq!(
            strip_jsonc(r#"{"a":"//not a comment",}"#),
            r#"{"a":"//not a comment"}"#
        );
    }
}
