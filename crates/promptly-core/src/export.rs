//! A session as a Markdown document: the conversation from its transcript
//! and the changes it made, for sharing.

use crate::git::{FileDiff, LineKind};
use crate::transcript::{Step, StepKind, steps_from_line};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Header facts shown under the title.
pub struct Meta<'a> {
    pub title: &'a str,
    pub folder: &'a str,
    pub branch: Option<&'a str>,
    pub model: Option<&'a str>,
    pub account: Option<&'a str>,
}

/// Every step in a transcript file (empty if it isn't there yet: Claude
/// Code writes it with a delay).
pub fn read_steps(transcript: &Path) -> Vec<Step> {
    let Ok(f) = std::fs::File::open(transcript) else {
        return vec![];
    };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .flat_map(|v| steps_from_line(&v))
        .collect()
}

/// The conversation (prompts, replies, tool calls; not hidden thinking)
/// followed by the changes, as Markdown.
pub fn markdown(meta: &Meta<'_>, steps: &[Step], changes: &[FileDiff]) -> String {
    let mut md = format!("# {}\n\n", meta.title);
    let mut facts = vec![format!("Folder: `{}`", meta.folder)];
    if let Some(b) = meta.branch {
        facts.push(format!("Branch: `{b}`"));
    }
    if let Some(m) = meta.model {
        facts.push(format!("Model: {m}"));
    }
    if let Some(a) = meta.account {
        facts.push(format!("Account: {a}"));
    }
    md.push_str(&facts.join(" · "));
    md.push_str("\n\n---\n\n");
    for step in steps {
        match step.kind {
            StepKind::Prompt => {
                md.push_str("## You\n\n");
                md.push_str(step.text.trim());
                md.push_str("\n\n");
            }
            StepKind::Reply => {
                md.push_str("## Claude\n\n");
                md.push_str(step.text.trim());
                md.push_str("\n\n");
            }
            StepKind::Tool => {
                md.push_str(&format!("> {}\n\n", step.text.trim().replace('\n', " ")));
            }
            StepKind::Thinking | StepKind::ThinkingHidden => {}
        }
    }
    if !changes.is_empty() {
        md.push_str("---\n\n## Changes\n\n");
        for f in changes {
            md.push_str(&format!(
                "### `{}` (+{} −{})\n\n```diff\n",
                f.path, f.added, f.removed
            ));
            for l in &f.lines {
                let sign = match l.kind {
                    LineKind::Added => "+",
                    LineKind::Removed => "-",
                    LineKind::Context => " ",
                    LineKind::Hunk | LineKind::Meta => "",
                };
                md.push_str(sign);
                md.push_str(&l.text);
                md.push('\n');
            }
            md.push_str("```\n\n");
        }
    }
    md
}

/// A file name for the export: "promptly-<title>-<date>.md".
pub fn file_name(title: &str, date: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "promptly-{}-{date}.md",
        if slug.is_empty() {
            "session".into()
        } else {
            slug
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffLine, FileDiff};

    #[test]
    fn conversation_and_changes() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("t.jsonl");
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Fix the login bug"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"secret"},{"type":"tool_use","name":"Read","input":{"file_path":"/a/login.ts"}},{"type":"text","text":"Fixed: the token check was inverted."}]}}"#,
        ];
        std::fs::write(&t, lines.join("\n")).unwrap();
        let changes = vec![FileDiff {
            path: "login.ts".into(),
            added: 1,
            removed: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Hunk,
                    text: "@@ -1 +1 @@".into(),
                    old_no: None,
                    new_no: None,
                },
                DiffLine {
                    kind: LineKind::Removed,
                    text: "if (!ok) allow()".into(),
                    old_no: Some(1),
                    new_no: None,
                },
                DiffLine {
                    kind: LineKind::Added,
                    text: "if (ok) allow()".into(),
                    old_no: None,
                    new_no: Some(1),
                },
            ],
            ..Default::default()
        }];
        assert!(read_steps(&d.path().join("missing.jsonl")).is_empty());
        let md = markdown(
            &Meta {
                title: "Login fix",
                folder: "~/shop",
                branch: Some("fix/login"),
                model: Some("Opus 5.5"),
                account: None,
            },
            &read_steps(&t),
            &changes,
        );
        assert!(md.starts_with("# Login fix\n"));
        assert!(md.contains("Branch: `fix/login`"));
        assert!(md.contains("## You\n\nFix the login bug"));
        assert!(md.contains("## Claude\n\nFixed: the token check was inverted."));
        assert!(md.contains("> "), "tool calls are listed");
        assert!(!md.contains("secret"), "thinking is not exported");
        assert!(md.contains("```diff\n@@ -1 +1 @@\n-if (!ok) allow()\n+if (ok) allow()\n```"));
        assert_eq!(
            file_name("Login fix: v2!", "2026-10-12"),
            "promptly-login-fix-v2-2026-10-12.md"
        );
    }
}
