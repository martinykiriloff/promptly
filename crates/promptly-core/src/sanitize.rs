//! Defenses against hostile terminal output and hostile pastes.

/// Window/tab titles come from untrusted program output: strip control
/// characters (including bidi overrides) and cap length.
pub fn title(raw: &str) -> String {
    one_line(raw, 80)
}

pub fn one_line(raw: &str, max: usize) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect();
    if s.chars().count() > max {
        s = s.chars().take(max.saturating_sub(1)).collect::<String>() + "…";
    }
    s
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

/// What a paste contains that the user should see before it reaches the PTY.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PasteRisk {
    pub lines: usize,
    /// Control characters other than newline, carriage return and tab.
    pub control_chars: usize,
    /// Contains a bracketed-paste terminator that could break out of paste mode.
    pub paste_terminator: bool,
}

impl PasteRisk {
    pub fn of(text: &str) -> Self {
        Self {
            lines: text.lines().count(),
            control_chars: text
                .chars()
                .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                .count(),
            paste_terminator: text.contains("\x1b[201~"),
        }
    }

    /// Show the preview when the paste carries control characters, or is
    /// multi-line and the program has not enabled bracketed paste (each
    /// newline would execute).
    pub fn needs_preview(&self, bracketed: bool) -> bool {
        self.control_chars > 0 || self.paste_terminator || (!bracketed && self.lines > 1)
    }
}

/// Remove control characters a paste should never carry.
pub fn strip_paste_controls(text: &str) -> String {
    text.replace("\x1b[201~", "")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

/// Encode a paste for the PTY. In bracketed mode the payload is wrapped and
/// any embedded terminator is removed so content cannot escape paste mode.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text
        .replace("\x1b[201~", "")
        .replace("\r\n", "\r")
        .replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{body}\x1b[201~").into_bytes()
    } else {
        body.into_bytes()
    }
}

/// Render control characters visibly for the paste preview.
pub fn visible_controls(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => "↵\n".to_string(),
            '\t' => "⇥".to_string(),
            '\x1b' => "␛".to_string(),
            c if c.is_control() => format!("^{}", ((c as u8) ^ 0x40) as char),
            c => c.to_string(),
        })
        .collect()
}

/// Redact tokens that look like secrets before logs are exported.
pub fn redact_secrets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split_inclusive(|c: char| c.is_whitespace()) {
        let trimmed = word.trim_end();
        let tail = &word[trimmed.len()..];
        let redacted = if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.to_ascii_uppercase();
            if ["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD", "AUTH"]
                .iter()
                .any(|s| key.contains(s))
                && !v.is_empty()
            {
                format!("{k}=[REDACTED]")
            } else {
                trimmed.to_string()
            }
        } else if looks_like_secret(trimmed) {
            "[REDACTED]".to_string()
        } else {
            trimmed.to_string()
        };
        out.push_str(&redacted);
        out.push_str(tail);
    }
    out
}

fn looks_like_secret(w: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "glpat-",
    ];
    PREFIXES
        .iter()
        .any(|p| w.starts_with(p) && w.len() >= p.len() + 12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_controls_and_bidi() {
        assert_eq!(
            title("ok\x1b]0;evil\x07\u{202E}gnp.exe"),
            "ok]0;evilgnp.exe"
        );
    }

    #[test]
    fn bracketed_paste_cannot_be_escaped() {
        let enc = encode_paste("a\x1b[201~rm -rf ~\n", true);
        assert_eq!(enc, b"\x1b[200~arm -rf ~\r\x1b[201~");
    }

    #[test]
    fn paste_risk() {
        assert!(!PasteRisk::of("one line").needs_preview(false));
        assert!(PasteRisk::of("a\nb").needs_preview(false));
        assert!(!PasteRisk::of("a\nb").needs_preview(true));
        assert!(PasteRisk::of("a\x1bb").needs_preview(true));
    }

    #[test]
    fn redaction() {
        assert_eq!(
            redact_secrets("GITHUB_TOKEN=abc123 ok sk-ant-aaaaaaaaaaaaaaaa"),
            "GITHUB_TOKEN=[REDACTED] ok [REDACTED]"
        );
    }
}
