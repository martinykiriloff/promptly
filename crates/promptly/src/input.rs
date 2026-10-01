//! Keyboard encoding: legacy xterm sequences, plus the Kitty keyboard
//! protocol's disambiguation mode when the program has enabled it (which is
//! how Shift+Enter reaches Claude Code as a distinct key).

use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers};

fn mod_param(m: Modifiers) -> u8 {
    1 + (m.shift as u8) + ((m.alt as u8) << 1) + ((m.ctrl as u8) << 2)
}

/// Encode a key press, or `None` when the key produces text that will
/// arrive separately as an `Event::Text`.
pub fn encode_key(
    key: Key,
    m: Modifiers,
    mode: TermMode,
    claude_pane: bool,
    option_as_meta: bool,
) -> Option<Vec<u8>> {
    let kitty = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    let all_as_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let mp = mod_param(m);
    let has_mods = mp > 1;
    let csi_u = |code: u32| {
        if mp > 1 {
            format!("\x1b[{code};{mp}u")
        } else {
            format!("\x1b[{code}u")
        }
        .into_bytes()
    };
    let alt_meta = m.alt && (option_as_meta || !cfg!(target_os = "macos"));

    // Keys with fixed codepoints in the Kitty protocol.
    let special = match key {
        Key::Enter => Some((13u32, b"\r".as_slice())),
        Key::Tab => Some((9, b"\t".as_slice())),
        Key::Backspace => Some((127, b"\x7f".as_slice())),
        Key::Escape => Some((27, b"\x1b".as_slice())),
        _ => None,
    };
    if let Some((code, legacy)) = special {
        if kitty && (has_mods || all_as_esc || key == Key::Escape) {
            return Some(csi_u(code));
        }
        return Some(match key {
            // Without the Kitty protocol, Claude Code reads ESC+CR as "newline".
            Key::Enter if m.shift && claude_pane => b"\x1b\r".to_vec(),
            Key::Enter if alt_meta => b"\x1b\r".to_vec(),
            Key::Tab if m.shift => b"\x1b[Z".to_vec(),
            Key::Backspace if m.ctrl => b"\x08".to_vec(),
            Key::Backspace if alt_meta => b"\x1b\x7f".to_vec(),
            _ => legacy.to_vec(),
        });
    }

    let cursor = |c: char| {
        Some(if has_mods {
            format!("\x1b[1;{mp}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        })
    };
    let tilde = |n: u8| {
        Some(
            if has_mods {
                format!("\x1b[{n};{mp}~")
            } else {
                format!("\x1b[{n}~")
            }
            .into_bytes(),
        )
    };
    let ss3_fn = |c: char| {
        Some(
            if has_mods {
                format!("\x1b[1;{mp}{c}")
            } else {
                format!("\x1bO{c}")
            }
            .into_bytes(),
        )
    };
    match key {
        Key::ArrowUp => return cursor('A'),
        Key::ArrowDown => return cursor('B'),
        Key::ArrowRight => return cursor('C'),
        Key::ArrowLeft => return cursor('D'),
        Key::Home => return cursor('H'),
        Key::End => return cursor('F'),
        Key::Insert => return tilde(2),
        Key::Delete => return tilde(3),
        Key::PageUp => return tilde(5),
        Key::PageDown => return tilde(6),
        Key::F1 => return ss3_fn('P'),
        Key::F2 => return ss3_fn('Q'),
        Key::F3 => return ss3_fn('R'),
        Key::F4 => return ss3_fn('S'),
        Key::F5 => return tilde(15),
        Key::F6 => return tilde(17),
        Key::F7 => return tilde(18),
        Key::F8 => return tilde(19),
        Key::F9 => return tilde(20),
        Key::F10 => return tilde(21),
        Key::F11 => return tilde(23),
        Key::F12 => return tilde(24),
        _ => {}
    }

    // Ctrl / Alt combinations with printable keys.
    let ch = key_char(key, m.shift)?;
    if m.ctrl {
        if kitty {
            return Some(csi_u(ch.to_ascii_lowercase() as u32));
        }
        let c = ch.to_ascii_lowercase();
        let byte = match c {
            'a'..='z' => (c as u8) & 0x1f,
            ' ' | '2' | '@' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '/' | '_' | '7' | '-' => 0x1f,
            '8' => 0x7f,
            _ => return None,
        };
        let mut v = Vec::new();
        if alt_meta {
            v.push(0x1b);
        }
        v.push(byte);
        return Some(v);
    }
    if alt_meta {
        if kitty {
            return Some(csi_u(ch.to_ascii_lowercase() as u32));
        }
        let mut v = vec![0x1b];
        v.extend(ch.to_string().as_bytes());
        return Some(v);
    }
    None
}

/// Whether a key press with these modifiers will be followed by a Text
/// event we must suppress (because we already encoded it).
pub fn suppresses_text(m: Modifiers, option_as_meta: bool) -> bool {
    m.ctrl || (m.alt && (option_as_meta || !cfg!(target_os = "macos")))
}

fn key_char(key: Key, shift: bool) -> Option<char> {
    let name = key.symbol_or_name();
    let mut chars = name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return match key {
            Key::Space => Some(' '),
            _ => None,
        };
    }
    Some(if shift {
        c.to_ascii_uppercase()
    } else {
        c.to_ascii_lowercase()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }
    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn legacy() {
        let m = TermMode::empty();
        assert_eq!(
            encode_key(Key::Enter, NONE, m, false, false).unwrap(),
            b"\r"
        );
        assert_eq!(
            encode_key(Key::C, ctrl(), m, false, false).unwrap(),
            b"\x03"
        );
        assert_eq!(
            encode_key(Key::ArrowUp, NONE, m, false, false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(Key::ArrowUp, NONE, TermMode::APP_CURSOR, false, false).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(Key::ArrowLeft, ctrl(), m, false, false).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(Key::Tab, shift(), m, false, false).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            encode_key(Key::F5, NONE, m, false, false).unwrap(),
            b"\x1b[15~"
        );
        assert!(
            encode_key(Key::A, NONE, m, false, false).is_none(),
            "plain text comes via Text"
        );
    }

    #[test]
    fn shift_enter() {
        let kitty = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(
            encode_key(Key::Enter, shift(), kitty, true, false).unwrap(),
            b"\x1b[13;2u"
        );
        assert_eq!(
            encode_key(Key::Enter, NONE, kitty, true, false).unwrap(),
            b"\r"
        );
        assert_eq!(
            encode_key(Key::Escape, NONE, kitty, true, false).unwrap(),
            b"\x1b[27u"
        );
        assert_eq!(
            encode_key(Key::Enter, shift(), TermMode::empty(), true, false).unwrap(),
            b"\x1b\r"
        );
        assert_eq!(
            encode_key(Key::C, ctrl(), kitty, true, false).unwrap(),
            b"\x1b[99;5u"
        );
    }
}
