//! Cross-session history: an SQLite FTS5 index over every Claude Code
//! transcript under `~/.claude/projects`, rebuilt incrementally by mtime.

use crate::transcript::{clean_prompt, message_text};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// v2: injected IDE/CLI tags are stripped from prompts.
const SCHEMA_VERSION: i64 = 2;

/// Cap on indexed text per session, so a huge transcript cannot bloat the DB.
const MAX_BODY: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub session_id: String,
    pub cwd: String,
    pub path: PathBuf,
    pub first_prompt: String,
    pub last_timestamp: String,
    pub turns: i64,
}

pub struct SessionIndex {
    db: Connection,
}

impl SessionIndex {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS sessions(
                 session_id TEXT PRIMARY KEY, cwd TEXT NOT NULL, path TEXT NOT NULL,
                 mtime INTEGER NOT NULL, first_prompt TEXT NOT NULL,
                 last_timestamp TEXT NOT NULL, turns INTEGER NOT NULL);
             CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                 session_id UNINDEXED, cwd, body, tokenize='unicode61');",
        )?;
        // Bump when parsing changes so existing rows are rebuilt once.
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            db.execute_batch(&format!(
                "DELETE FROM sessions; DELETE FROM sessions_fts; PRAGMA user_version = {SCHEMA_VERSION};"
            ))?;
        }
        Ok(Self { db })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    /// Index new or changed transcripts. Returns how many were (re)indexed.
    pub fn refresh(&mut self, projects_dir: &Path) -> rusqlite::Result<usize> {
        let mut files = Vec::new();
        if let Ok(dirs) = std::fs::read_dir(projects_dir) {
            for d in dirs.flatten() {
                if let Ok(entries) = std::fs::read_dir(d.path()) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.extension().is_some_and(|x| x == "jsonl") {
                            files.push(p);
                        }
                    }
                }
            }
        }
        let mut n = 0;
        for p in files {
            let Some(mtime) = mtime_secs(&p) else {
                continue;
            };
            let id = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let known: Option<i64> = self
                .db
                .query_row(
                    "SELECT mtime FROM sessions WHERE session_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .ok();
            if known == Some(mtime) {
                continue;
            }
            if let Some(parsed) = parse_transcript(&p) {
                self.upsert(&id, &p, mtime, &parsed)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn upsert(&mut self, id: &str, path: &Path, mtime: i64, t: &Parsed) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO sessions VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                t.cwd,
                path.to_string_lossy(),
                mtime,
                t.first_prompt,
                t.last_timestamp,
                t.turns
            ],
        )?;
        tx.execute("DELETE FROM sessions_fts WHERE session_id=?1", [id])?;
        tx.execute(
            "INSERT INTO sessions_fts(session_id, cwd, body) VALUES (?1,?2,?3)",
            params![id, t.cwd, t.body],
        )?;
        tx.commit()
    }

    /// Full-text search; an empty query lists the most recent sessions.
    pub fn search(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<SessionRecord>> {
        let q = fts_query(query);
        let map = |r: &rusqlite::Row<'_>| {
            Ok(SessionRecord {
                session_id: r.get(0)?,
                cwd: r.get(1)?,
                path: PathBuf::from(r.get::<_, String>(2)?),
                first_prompt: r.get(3)?,
                last_timestamp: r.get(4)?,
                turns: r.get(5)?,
            })
        };
        let cols = "s.session_id, s.cwd, s.path, s.first_prompt, s.last_timestamp, s.turns";
        if q.is_empty() {
            let mut st = self.db.prepare(&format!(
                "SELECT {cols} FROM sessions s ORDER BY s.last_timestamp DESC LIMIT ?1"
            ))?;
            let rows = st.query_map([limit as i64], map)?;
            rows.collect()
        } else {
            let mut st = self.db.prepare(&format!(
                "SELECT {cols} FROM sessions_fts f JOIN sessions s ON s.session_id=f.session_id
                 WHERE sessions_fts MATCH ?1 ORDER BY bm25(sessions_fts), s.last_timestamp DESC LIMIT ?2"
            ))?;
            let rows = st.query_map(params![q, limit as i64], map)?;
            rows.collect()
        }
    }
}

/// Turn user input into a safe FTS5 query: every word becomes a quoted
/// prefix term, so punctuation can never be parsed as FTS syntax.
fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

struct Parsed {
    cwd: String,
    first_prompt: String,
    last_timestamp: String,
    turns: i64,
    body: String,
}

fn parse_transcript(p: &Path) -> Option<Parsed> {
    let f = std::fs::File::open(p).ok()?;
    let mut out = Parsed {
        cwd: String::new(),
        first_prompt: String::new(),
        last_timestamp: String::new(),
        turns: 0,
        body: String::new(),
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        if out.cwd.is_empty()
            && let Some(c) = v["cwd"].as_str()
        {
            out.cwd = c.to_string();
        }
        if let Some(ts) = v["timestamp"].as_str() {
            out.last_timestamp = ts.to_string();
        }
        let ty = v["type"].as_str().unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }
        let Some(text) = message_text(&v["message"]["content"]) else {
            continue;
        };
        if ty == "user" {
            let Some(clean) = clean_prompt(&text) else {
                continue;
            };
            out.turns += 1;
            if out.first_prompt.is_empty() {
                out.first_prompt = crate::sanitize::one_line(&clean, 200);
            }
        }
        if out.body.len() < MAX_BODY {
            out.body.push_str(&text);
            out.body.push('\n');
        }
    }
    (!out.cwd.is_empty() || out.turns > 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-work-api");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/transcript.jsonl"
            ),
            proj.join("11111111-2222-4333-8444-555555555555.jsonl"),
        )
        .unwrap();
        let mut idx = SessionIndex::open_in_memory().unwrap();
        assert_eq!(idx.refresh(dir.path()).unwrap(), 1);
        assert_eq!(
            idx.refresh(dir.path()).unwrap(),
            0,
            "unchanged mtime is skipped"
        );
        let hits = idx.search("health", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cwd, "/work/api");
        assert_eq!(hits[0].first_prompt, "add a health endpoint");
        assert_eq!(idx.search("heal", 10).unwrap().len(), 1, "prefix match");
        assert!(idx.search("nonexistent", 10).unwrap().is_empty());
        assert_eq!(
            idx.search("\"unbalanced OR (", 10).unwrap().len(),
            0,
            "hostile query is safe"
        );
        assert_eq!(idx.search("", 10).unwrap().len(), 1);
    }
}
