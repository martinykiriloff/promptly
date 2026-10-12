//! Claude Code's permission prompt, read off the terminal screen so
//! Promptly can offer the same choices as buttons. Promptly only ever
//! presses a choice the user clicked; it never answers on its own.

/// A permission prompt as it appears on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The question, e.g. "Do you want to proceed?".
    pub question: String,
    /// What Claude wants to do (the lines between the header and the
    /// question, trimmed), e.g. "touch notes.txt".
    pub request: Vec<String>,
    /// Numbered choices in Claude's order: (number, label).
    pub options: Vec<(u8, String)>,
}

impl Prompt {
    /// The keys that pick `number` (Claude selects and confirms on a digit).
    pub fn keys_for(&self, number: u8) -> Option<Vec<u8>> {
        self.options
            .iter()
            .any(|(n, _)| *n == number)
            .then(|| vec![b'0' + number])
    }

    /// The choice that refuses ("No…"), if listed.
    pub fn deny_option(&self) -> Option<u8> {
        self.options
            .iter()
            .find(|(_, l)| l.starts_with("No"))
            .map(|(n, _)| *n)
    }

    /// The plain "Yes".
    pub fn approve_option(&self) -> Option<u8> {
        self.options
            .iter()
            .find(|(_, l)| l == "Yes")
            .map(|(n, _)| *n)
    }
}

fn option_line(line: &str) -> Option<(u8, String)> {
    let t = line.trim().trim_start_matches(['❯', '>', '›']).trim();
    let (num, rest) = t.split_once(". ")?;
    let n: u8 = num.parse().ok()?;
    (1..=9).contains(&n).then(|| (n, rest.trim().to_string()))
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '╌' | '━' | '-' | '═'))
}

/// Find the permission prompt on screen (the last one, if several).
pub fn parse(screen: &str) -> Option<Prompt> {
    let lines: Vec<&str> = screen.lines().collect();
    let q = lines.iter().rposition(|l| {
        let t = l.trim();
        t.starts_with("Do you want to") && t.ends_with('?')
    })?;
    let mut options: Vec<(u8, String)> = vec![];
    for l in &lines[q + 1..] {
        let t = l.trim();
        if let Some(o) = option_line(l) {
            options.push(o);
        } else if t.is_empty() {
            continue;
        } else if t.starts_with("Esc") || t.starts_with("Tab to") || is_rule(l) {
            break;
        } else if let Some(last) = options.last_mut() {
            // A long choice wraps onto the next line.
            last.1.push(' ');
            last.1.push_str(t);
        } else {
            break;
        }
    }
    if options.len() < 2 {
        return None;
    }
    // The request: lines above the question, back to the box's top rule.
    let mut request = vec![];
    for l in lines[..q].iter().rev() {
        if l.trim().starts_with("──") && is_rule(l) {
            break;
        }
        let t = l.trim();
        if t.is_empty() || is_rule(l) || t.starts_with("Tip:") {
            continue;
        }
        request.push(t.to_string());
    }
    request.reverse();
    Some(Prompt {
        question: lines[q].trim().to_string(),
        request,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from Claude Code 2.1.296.
    const BASH: &str = "\
❯ Run exactly this shell command with the Bash tool and nothing else: touch probe-approved.txt
⏺ Creating probe-approved.txt file
  ⎿  $ touch probe-approved.txt
──────────────────────────────────────────────────────────────
 Bash command
 Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below
 Create probe-approved.txt file
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 touch probe-approved.txt
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to /Users/me from this project
   3. Yes, and switch to auto mode · auto mode handles these prompts for you
   4. No
 Esc to cancel · Tab to amend
";

    #[test]
    fn parses_claude_prompt() {
        let p = parse(BASH).expect("prompt");
        assert_eq!(p.question, "Do you want to proceed?");
        assert_eq!(
            p.request,
            [
                "Bash command",
                "Create probe-approved.txt file",
                "touch probe-approved.txt"
            ]
        );
        assert_eq!(p.options.len(), 4);
        assert_eq!(p.options[1].0, 2);
        assert!(p.options[1].1.starts_with("Yes, and always allow"));
        assert_eq!(p.approve_option(), Some(1));
        assert_eq!(p.deny_option(), Some(4));
        assert_eq!(p.keys_for(1), Some(b"1".to_vec()));
        assert_eq!(p.keys_for(7), None, "only listed choices");
    }

    #[test]
    fn three_option_edit_prompt() {
        let screen = "\
──────────────────────────
 Edit file
 src/main.rs
 Do you want to make this edit to main.rs?
 ❯ 1. Yes
   2. Yes, allow all edits during this session (shift+tab)
   3. No, and tell Claude what to do differently (esc)
";
        let p = parse(screen).unwrap();
        assert_eq!(p.deny_option(), Some(3));
        assert_eq!(p.request, ["Edit file", "src/main.rs"]);
    }

    #[test]
    fn wrapped_choices_are_joined() {
        let screen = "\
──────────────────────────
 Bash command
 touch denied.txt
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to
   deny-1760000000000/ from this project
   3. No

 Esc to cancel · Tab to amend
";
        let p = parse(screen).unwrap();
        assert_eq!(p.options.len(), 3);
        assert_eq!(
            p.options[1].1,
            "Yes, and always allow access to deny-1760000000000/ from this project"
        );
        assert_eq!(p.deny_option(), Some(3));
    }

    #[test]
    fn nothing_without_a_prompt() {
        assert_eq!(parse("❯ hello\n⏺ Hi there\n"), None);
        // A numbered list in Claude's answer isn't a prompt.
        assert_eq!(parse("Steps:\n1. Install\n2. Run\n"), None);
    }
}
