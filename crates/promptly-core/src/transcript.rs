//! Channel 3: transcript tailing. Watches the session JSONL with FSEvents /
//! inotify (via `notify`) and parses it defensively: unknown line types and
//! fields are skipped, never fatal.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptActivity {
    Working,
    TurnEnded,
    Unknown,
}

/// Running totals derived from the transcript.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptStats {
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Tokens in the context window as of the last assistant message.
    pub context_tokens: u64,
    pub output_tokens: u64,
    pub user_turns: u32,
    pub tool_calls: u32,
    pub first_prompt: Option<String>,
    pub last_activity: Option<TranscriptActivity>,
}

impl TranscriptStats {
    /// Context window size for the model. `[1m]` variants get the long window.
    pub fn context_window(&self) -> u64 {
        match self.model.as_deref() {
            Some(m) if m.contains("[1m]") || m.ends_with("-1m") => 1_000_000,
            // A session already past 200k must be running with the long window.
            _ if self.context_tokens > 200_000 => 1_000_000,
            _ => 200_000,
        }
    }

    pub fn context_percent(&self) -> Option<f32> {
        (self.context_tokens > 0)
            .then(|| (self.context_tokens as f32 / self.context_window() as f32 * 100.0).min(100.0))
    }

    /// Fold one JSONL line into the stats. Returns the activity it implies.
    pub fn ingest_line(&mut self, line: &str) -> TranscriptActivity {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return TranscriptActivity::Unknown;
        };
        if let Some(id) = v.get("sessionId").and_then(Value::as_str) {
            self.session_id.get_or_insert_with(|| id.to_string());
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return TranscriptActivity::Unknown;
        }
        let activity = match v.get("type").and_then(Value::as_str) {
            Some("user") => self.ingest_user(&v),
            Some("assistant") => self.ingest_assistant(&v),
            _ => TranscriptActivity::Unknown,
        };
        if activity != TranscriptActivity::Unknown {
            self.last_activity = Some(activity);
        }
        activity
    }

    fn ingest_user(&mut self, v: &Value) -> TranscriptActivity {
        let content = &v["message"]["content"];
        // Tool results arrive as user messages; they are not human turns.
        let is_tool_result = content.as_array().is_some_and(|a| {
            a.iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        });
        if !is_tool_result {
            self.user_turns += 1;
            if self.first_prompt.is_none() {
                self.first_prompt = message_text(content)
                    .and_then(|t| clean_prompt(&t))
                    .map(|t| crate::sanitize::one_line(&t, 200));
            }
        }
        TranscriptActivity::Working
    }

    fn ingest_assistant(&mut self, v: &Value) -> TranscriptActivity {
        let msg = &v["message"];
        if let Some(m) = msg.get("model").and_then(Value::as_str)
            && m != "<synthetic>"
        {
            self.model = Some(m.to_string());
        }
        if let Some(u) = msg.get("usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let ctx =
                n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
            if ctx > 0 {
                self.context_tokens = ctx + n("output_tokens");
            }
            self.output_tokens += n("output_tokens");
        }
        if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
            self.tool_calls += blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                .count() as u32;
        }
        match msg.get("stop_reason").and_then(Value::as_str) {
            Some("end_turn") | Some("stop_sequence") | Some("max_tokens") => {
                TranscriptActivity::TurnEnded
            }
            _ => TranscriptActivity::Working,
        }
    }
}

/// One step of Claude's process, in order: what you asked, what it thought,
/// what it did, what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    Prompt,
    Thinking,
    /// Claude thought, but the text isn't in the transcript (the model or
    /// CLI version didn't return it).
    ThinkingHidden,
    Tool,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    pub text: String,
    /// RFC 3339 timestamp from the transcript, when present.
    pub at: Option<String>,
}

/// Steps contained in one transcript line. Sub-agent (sidechain) lines and
/// tool results are skipped; they belong to other views.
pub fn steps_from_line(v: &Value) -> Vec<Step> {
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return vec![];
    }
    let at = v
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let step = |kind, text: String| Step {
        kind,
        text,
        at: at.clone(),
    };
    let content = &v["message"]["content"];
    match v.get("type").and_then(Value::as_str) {
        Some("user") => {
            let is_tool_result = content.as_array().is_some_and(|a| {
                a.iter()
                    .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            });
            if is_tool_result {
                return vec![];
            }
            message_text(content)
                .and_then(|t| clean_prompt(&t))
                .map(|t| vec![step(StepKind::Prompt, t)])
                .unwrap_or_default()
        }
        Some("assistant") => {
            let Some(blocks) = content.as_array() else {
                return vec![];
            };
            blocks
                .iter()
                .filter_map(|b| match b.get("type").and_then(Value::as_str)? {
                    "thinking" => {
                        let t = b
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim();
                        Some(if t.is_empty() {
                            step(StepKind::ThinkingHidden, String::new())
                        } else {
                            step(StepKind::Thinking, t.to_string())
                        })
                    }
                    "redacted_thinking" => Some(step(StepKind::ThinkingHidden, String::new())),
                    "tool_use" => {
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let input = b.get("input");
                        let detail = input.and_then(|i| {
                            [
                                "description",
                                "command",
                                "file_path",
                                "path",
                                "pattern",
                                "url",
                                "query",
                                "prompt",
                            ]
                            .iter()
                            .find_map(|k| i.get(*k).and_then(Value::as_str))
                        });
                        let text = match detail {
                            Some(d) => format!("{name}: {}", crate::sanitize::one_line(d, 140)),
                            None => name.to_string(),
                        };
                        Some(step(StepKind::Tool, text))
                    }
                    "text" => {
                        let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        (!t.is_empty()).then(|| step(StepKind::Reply, t.to_string()))
                    }
                    _ => None,
                })
                .collect()
        }
        _ => vec![],
    }
}

/// Remove blocks the CLI or IDE injects into user messages
/// (`<ide_selection>…</ide_selection>`, `<command-name>…`, `<system-reminder>…`).
/// Returns `None` when nothing the human typed is left.
pub fn clean_prompt(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        let close = format!("</{name}>");
        match (
            name.is_empty(),
            after[name_len..].starts_with('>'),
            rest.find(&close),
        ) {
            (false, true, Some(end)) => {
                out.push_str(&rest[..start]);
                rest = &rest[end + close.len()..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    let t = out.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Plain text of a message `content`, which is a string or a block array.
pub fn message_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let t: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!t.is_empty()).then(|| t.join("\n"))
        }
        _ => None,
    }
}

/// Incremental reader: remembers its offset and only parses complete lines.
pub struct TranscriptReader {
    path: PathBuf,
    offset: u64,
    partial: String,
    pub stats: TranscriptStats,
    /// Steps read since the last `take_steps`.
    steps: Vec<Step>,
}

impl TranscriptReader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: String::new(),
            stats: TranscriptStats::default(),
            steps: Vec::new(),
        }
    }

    /// Steps parsed since the previous call.
    pub fn take_steps(&mut self) -> Vec<Step> {
        std::mem::take(&mut self.steps)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read whatever was appended since the last call. Returns the most
    /// recent activity seen in the new lines.
    pub fn poll(&mut self) -> std::io::Result<Option<TranscriptActivity>> {
        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let len = f.metadata()?.len();
        if len < self.offset {
            // Truncated or replaced: start over.
            self.offset = 0;
            self.partial.clear();
            self.stats = TranscriptStats::default();
            self.steps.clear();
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(f);
        let mut last = None;
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf)?;
            if n == 0 {
                break;
            }
            self.offset += n as u64;
            if !buf.ends_with('\n') {
                self.partial.push_str(&buf);
                continue;
            }
            let line = if self.partial.is_empty() {
                buf.trim_end().to_string()
            } else {
                let mut l = std::mem::take(&mut self.partial);
                l.push_str(buf.trim_end());
                l
            };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                self.steps.extend(steps_from_line(&v));
            }
            let a = self.stats.ingest_line(&line);
            if a != TranscriptActivity::Unknown {
                last = Some(a);
            }
        }
        Ok(last)
    }
}

/// Watches a transcript's directory and yields a tick whenever it changes.
/// The directory is watched (not the file) because the file may not exist
/// yet when the session starts.
pub struct TranscriptWatch {
    _watcher: RecommendedWatcher,
    pub ticks: Receiver<()>,
}

impl TranscriptWatch {
    pub fn new(path: &Path) -> notify::Result<Self> {
        let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        std::fs::create_dir_all(&dir).ok();
        let (tx, rx) = mpsc::channel();
        let name = path.file_name().map(|n| n.to_os_string());
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res
                    && ev
                        .paths
                        .iter()
                        .any(|p| p.file_name().map(|n| n.to_os_string()) == name)
                {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            ticks: rx,
        })
    }

    /// Block until a change or the timeout; drains coalesced ticks.
    pub fn wait(&self, timeout: Duration) -> bool {
        let got = self.ticks.recv_timeout(timeout).is_ok();
        while self.ticks.try_recv().is_ok() {}
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &str = include_str!("../tests/fixtures/transcript.jsonl");

    #[test]
    fn stats_from_fixture() {
        let mut s = TranscriptStats::default();
        let mut last = TranscriptActivity::Unknown;
        for l in FIXTURE.lines() {
            let a = s.ingest_line(l);
            if a != TranscriptActivity::Unknown {
                last = a;
            }
        }
        assert_eq!(
            s.session_id.as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
        assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(s.user_turns, 1);
        assert_eq!(s.tool_calls, 1);
        assert_eq!(s.context_tokens, 2 + 4000 + 26000 + 50);
        assert_eq!(s.first_prompt.as_deref(), Some("add a health endpoint"));
        assert_eq!(last, TranscriptActivity::TurnEnded);
        assert!((s.context_percent().unwrap() - 15.026).abs() < 0.01);
    }

    #[test]
    fn reader_handles_partial_lines_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let mut f = File::create(&p).unwrap();
        let mut r = TranscriptReader::new(p.clone());
        write!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"hi"}}"#
        )
        .unwrap();
        assert_eq!(r.poll().unwrap(), None);
        writeln!(f, "}}").unwrap();
        assert_eq!(r.poll().unwrap(), Some(TranscriptActivity::Working));
        assert_eq!(r.stats.user_turns, 1);
        File::create(&p).unwrap();
        r.poll().unwrap();
        assert_eq!(r.stats.user_turns, 0);
    }

    #[test]
    fn injected_tags_are_not_prompts() {
        assert_eq!(
            clean_prompt("<ide_selection>The user selected lines 1-9</ide_selection> fix this")
                .as_deref(),
            Some("fix this")
        );
        assert_eq!(
            clean_prompt("<command-name>/clear</command-name>\n<command-args></command-args>"),
            None
        );
        assert_eq!(
            clean_prompt("is a < b and c > d?").as_deref(),
            Some("is a < b and c > d?")
        );
        let mut s = TranscriptStats::default();
        s.ingest_line(r#"{"type":"user","message":{"role":"user","content":"<ide_selection>x</ide_selection>"}}"#);
        s.ingest_line(r#"{"type":"user","message":{"role":"user","content":"real question"}}"#);
        assert_eq!(s.first_prompt.as_deref(), Some("real question"));
    }

    #[test]
    fn steps_follow_the_process() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"<ide_selection>x</ide_selection> add tests"},"timestamp":"2026-10-01T10:00:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"The parser lacks edge-case tests; start with empty input.","signature":"s"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"s"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test -q","description":"Run tests"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Added 3 tests."}]}}"#,
        ];
        let steps: Vec<Step> = lines
            .iter()
            .flat_map(|l| steps_from_line(&serde_json::from_str(l).unwrap()))
            .collect();
        let kinds: Vec<_> = steps.iter().map(|s| s.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                StepKind::Prompt,
                StepKind::Thinking,
                StepKind::ThinkingHidden,
                StepKind::Tool,
                StepKind::Reply
            ]
        );
        assert_eq!(steps[0].text, "add tests");
        assert_eq!(steps[0].at.as_deref(), Some("2026-10-01T10:00:00Z"));
        assert_eq!(steps[3].text, "Bash: Run tests");
    }

    #[test]
    fn reader_collects_steps_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, include_str!("../tests/fixtures/transcript.jsonl")).unwrap();
        let mut r = TranscriptReader::new(p);
        r.poll().unwrap();
        let s = r.take_steps();
        assert!(
            s.iter()
                .any(|x| x.kind == StepKind::Prompt && x.text == "add a health endpoint")
        );
        assert!(s.iter().any(|x| x.kind == StepKind::Tool));
        assert!(r.take_steps().is_empty(), "drained");
    }

    #[test]
    fn long_context_inferred_from_usage() {
        let s = TranscriptStats {
            context_tokens: 300_000,
            model: Some("claude-opus-5".into()),
            ..Default::default()
        };
        assert_eq!(s.context_window(), 1_000_000);
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let mut s = TranscriptStats::default();
        assert_eq!(s.ingest_line("not json"), TranscriptActivity::Unknown);
        assert_eq!(
            s.ingest_line(r#"{"type":"future-thing","x":1}"#),
            TranscriptActivity::Unknown
        );
    }
}
