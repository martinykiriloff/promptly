//! Print a summary of Claude's steps in a transcript.
//!   cargo run -q -p promptly-core --example steps -- <transcript.jsonl>
use promptly_core::transcript::{StepKind, TranscriptReader};
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).expect("transcript path");
    let mut r = TranscriptReader::new(path.into());
    r.poll().expect("read transcript");
    let steps = r.take_steps();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for s in &steps {
        *counts.entry(format!("{:?}", s.kind)).or_default() += 1;
    }
    println!("{counts:?}");
    if let Some(t) = steps.iter().rev().find(|s| s.kind == StepKind::Thinking) {
        let preview: String = t.text.chars().take(160).collect();
        println!("latest thinking ({} chars): {preview}…", t.text.len());
    }
}
