//! Saved prompts: reusable prompts with `{{variables}}`, for every project
//! or for one project.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPrompt {
    pub name: String,
    pub body: String,
    /// Only offered in this project (repository root or folder).
    #[serde(default)]
    pub project: Option<PathBuf>,
}

fn path() -> PathBuf {
    crate::paths::data_dir().join("prompts.json")
}

pub fn load() -> Vec<SavedPrompt> {
    load_from(&path())
}

pub fn save(all: &[SavedPrompt]) -> std::io::Result<()> {
    save_to(&path(), all)
}

fn load_from(p: &Path) -> Vec<SavedPrompt> {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_to(p: &Path, all: &[SavedPrompt]) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let body = serde_json::to_vec_pretty(all).map_err(std::io::Error::other)?;
    crate::util::write_private(p, &body)
}

/// Prompts offered in `project`: the global ones plus that project's.
pub fn for_project<'a>(all: &'a [SavedPrompt], project: Option<&Path>) -> Vec<&'a SavedPrompt> {
    all.iter()
        .filter(|p| match (&p.project, project) {
            (None, _) => true,
            (Some(pp), Some(cur)) => cur.starts_with(pp),
            (Some(_), None) => false,
        })
        .collect()
}

/// `{{name}}` placeholders, in order of first appearance.
pub fn variables(body: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            break;
        };
        let name = after[..j].trim();
        if !name.is_empty() && !name.contains('\n') && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &after[j + 2..];
    }
    out
}

/// Replace each `{{name}}` with its value (unknown names are left as is).
pub fn fill(body: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                let name = after[..j].trim();
                match values.get(name) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&rest[i..i + 2 + j + 2]),
                }
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_and_fill() {
        let body = "Review {{file}} for {{ concern }}. Focus on {{file}} only. {{}} {{open";
        assert_eq!(variables(body), ["file", "concern"]);
        let mut v = HashMap::new();
        v.insert("file".to_string(), "auth.ts".to_string());
        v.insert("concern".to_string(), "security".to_string());
        assert_eq!(
            fill(body, &v),
            "Review auth.ts for security. Focus on auth.ts only. {{}} {{open"
        );
        assert_eq!(fill("no vars", &v), "no vars");
    }

    #[test]
    fn project_scope_and_round_trip() {
        let all = vec![
            SavedPrompt {
                name: "Explain".into(),
                body: "Explain {{x}}".into(),
                project: None,
            },
            SavedPrompt {
                name: "Deploy".into(),
                body: "Deploy".into(),
                project: Some("/src/shop".into()),
            },
        ];
        let names = |p: Option<&Path>| -> Vec<String> {
            for_project(&all, p)
                .iter()
                .map(|x| x.name.clone())
                .collect()
        };
        assert_eq!(
            names(Some(Path::new("/src/shop/api"))),
            ["Explain", "Deploy"]
        );
        assert_eq!(names(Some(Path::new("/src/other"))), ["Explain"]);
        assert_eq!(names(None), ["Explain"]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("prompts.json");
        save_to(&p, &all).unwrap();
        assert_eq!(load_from(&p), all);
    }
}
