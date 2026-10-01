//! Slash commands and skills Claude Code understands in a given project:
//! built-ins, your own and the project's commands and skills, and those of
//! installed plugins (named `plugin:name`, as Claude Code shows them).

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Project,
    User,
    Plugin,
    BuiltIn,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Project => "project",
            Source::User => "user",
            Source::Plugin => "plugin",
            Source::BuiltIn => "built-in",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashItem {
    /// Without the leading slash, e.g. `compact` or `frontend-design:frontend-design`.
    pub name: String,
    pub description: String,
    pub source: Source,
    pub is_skill: bool,
}

/// Claude Code's built-in commands that are useful to send from a composer.
const BUILT_IN: &[(&str, &str)] = &[
    ("add-dir", "Add another working directory"),
    ("agents", "Manage custom subagents"),
    ("clear", "Clear the conversation and free up context"),
    ("compact", "Summarise the conversation to free up context"),
    ("config", "Open Claude Code settings"),
    ("context", "Show what is using the context window"),
    ("cost", "Show token usage and cost for this session"),
    ("doctor", "Check the health of your Claude Code install"),
    ("export", "Export the conversation"),
    ("help", "Show help and available commands"),
    ("hooks", "Manage hooks"),
    ("init", "Create a CLAUDE.md for this project"),
    ("login", "Sign in to your Anthropic account"),
    ("logout", "Sign out"),
    ("mcp", "Manage MCP servers"),
    ("memory", "Edit CLAUDE.md memory files"),
    ("model", "Choose the model"),
    ("permissions", "View and edit tool permissions"),
    ("plugin", "Manage plugins"),
    ("resume", "Resume a previous conversation"),
    ("review", "Review a pull request"),
    (
        "rewind",
        "Rewind the conversation or code to an earlier point",
    ),
    ("status", "Show version, model, account and connectivity"),
    ("statusline", "Set up the status line"),
    ("usage", "Show plan usage limits"),
];

/// Every command and skill available to a session in `cwd`, sorted with
/// project items first, then yours, plugins, and built-ins.
pub fn discover(cwd: &Path, claude_dir: &Path) -> Vec<SlashItem> {
    let mut items: Vec<SlashItem> = Vec::new();
    let root = crate::git::repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let project = root.join(".claude");

    commands_in(&project.join("commands"), None, Source::Project, &mut items);
    skills_in(&project.join("skills"), None, Source::Project, &mut items);
    commands_in(&claude_dir.join("commands"), None, Source::User, &mut items);
    skills_in(&claude_dir.join("skills"), None, Source::User, &mut items);
    for (plugin, path) in installed_plugins(claude_dir) {
        commands_in(
            &path.join("commands"),
            Some(&plugin),
            Source::Plugin,
            &mut items,
        );
        skills_in(
            &path.join("skills"),
            Some(&plugin),
            Source::Plugin,
            &mut items,
        );
    }
    for (name, desc) in BUILT_IN {
        items.push(SlashItem {
            name: (*name).into(),
            description: (*desc).into(),
            source: Source::BuiltIn,
            is_skill: false,
        });
    }
    // The first definition of a name wins (project shadows user, and so on).
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.name.clone()));
    items
}

/// Items matching what was typed after "/": name prefix first, then name
/// substring, then description, each group in discovery order.
pub fn filter<'a>(items: &'a [SlashItem], query: &str) -> Vec<&'a SlashItem> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return items.iter().collect();
    }
    let rank = |i: &SlashItem| {
        let n = i.name.to_ascii_lowercase();
        let short = n.rsplit(':').next().unwrap_or(&n).to_string();
        if n.starts_with(&q) || short.starts_with(&q) {
            Some(0)
        } else if n.contains(&q) {
            Some(1)
        } else if i.description.to_ascii_lowercase().contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    let mut v: Vec<(u8, usize, &SlashItem)> = items
        .iter()
        .enumerate()
        .filter_map(|(ix, i)| rank(i).map(|r| (r, ix, i)))
        .collect();
    v.sort_by_key(|(r, ix, _)| (*r, *ix));
    v.into_iter().map(|(_, _, i)| i).collect()
}

fn installed_plugins(claude_dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(bytes) = std::fs::read(claude_dir.join("plugins/installed_plugins.json")) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return vec![];
    };
    let Some(map) = v.get("plugins").and_then(Value::as_object) else {
        return vec![];
    };
    let mut out = Vec::new();
    for (key, installs) in map {
        let plugin = key.split('@').next().unwrap_or(key).to_string();
        // Newest install of this plugin.
        let path = installs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("installPath").and_then(Value::as_str))
            .next_back()
            .map(PathBuf::from);
        if let Some(p) = path.filter(|p| p.is_dir()) {
            out.push((plugin, p));
        }
    }
    out.sort();
    out
}

fn commands_in(dir: &Path, plugin: Option<&str>, source: Source, out: &mut Vec<SlashItem>) {
    let mut files = Vec::new();
    collect_md(dir, &mut files, 0);
    files.sort();
    for f in files {
        let Some(stem) = f.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Unreadable files (e.g. broken symlinks) aren't loaded by Claude Code either.
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let (fm, body) = front_matter(&text);
        let description = fm_value(&fm, "description").unwrap_or_else(|| first_line(body));
        let name = match plugin {
            Some(p) => format!("{p}:{stem}"),
            None => stem.to_string(),
        };
        out.push(SlashItem {
            name,
            description,
            source,
            is_skill: false,
        });
    }
}

fn skills_in(dir: &Path, plugin: Option<&str>, source: Source, out: &mut Vec<SlashItem>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SKILL.md").is_file())
        .collect();
    dirs.sort();
    for d in dirs {
        let Ok(text) = std::fs::read_to_string(d.join("SKILL.md")) else {
            continue;
        };
        let (fm, body) = front_matter(&text);
        let base = fm_value(&fm, "name").unwrap_or_else(|| {
            d.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let description = fm_value(&fm, "description").unwrap_or_else(|| first_line(body));
        let name = match plugin {
            Some(p) => format!("{p}:{base}"),
            None => base,
        };
        out.push(SlashItem {
            name,
            description,
            source,
            is_skill: true,
        });
    }
}

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 3 {
            collect_md(&p, out, depth + 1);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// Split `---` front matter from the body.
fn front_matter(text: &str) -> (Vec<String>, &str) {
    let Some(rest) = text.strip_prefix("---") else {
        return (vec![], text);
    };
    let Some(end) = rest.find("\n---") else {
        return (vec![], text);
    };
    let fm = rest[..end].lines().map(str::to_owned).collect();
    let body = rest[end + 4..].trim_start_matches(['-', '\n', '\r']);
    (fm, body)
}

/// Value of `key:` in front matter, including folded/indented continuations.
fn fm_value(fm: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let i = fm.iter().position(|l| l.starts_with(&prefix))?;
    let mut v = fm[i][prefix.len()..].trim().to_string();
    if v.is_empty() || v == ">" || v == "|" || v == ">-" || v == "|-" {
        v = fm[i + 1..]
            .iter()
            .take_while(|l| l.starts_with(' ') || l.starts_with('\t'))
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
    }
    let v = v.trim_matches(['"', '\'']).trim().to_string();
    (!v.is_empty()).then(|| crate::sanitize::one_line(&v, 200))
}

fn first_line(body: &str) -> String {
    let l = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("");
    crate::sanitize::one_line(l, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_all_sources_with_shadowing() {
        let t = tempfile::tempdir().unwrap();
        let claude = t.path().join("claude");
        let proj = t.path().join("proj");
        std::fs::create_dir_all(proj.join(".claude/commands")).unwrap();
        std::fs::write(
            proj.join(".claude/commands/deploy.md"),
            "---\ndescription: Ship it\n---\nDeploy now.",
        )
        .unwrap();
        std::fs::create_dir_all(claude.join("commands")).unwrap();
        std::fs::write(claude.join("commands/deploy.md"), "User deploy").unwrap();
        std::fs::write(
            claude.join("commands/commit.md"),
            "# Commit\nWrite a good commit message.",
        )
        .unwrap();
        std::fs::create_dir_all(claude.join("skills/seo")).unwrap();
        std::fs::write(
            claude.join("skills/seo/SKILL.md"),
            "---\nname: seo-audit\ndescription: >\n  Audit a site\n  for SEO\n---\nbody",
        )
        .unwrap();
        let plug = claude.join("plugins/cache/m/design/abc");
        std::fs::create_dir_all(plug.join("skills/frontend")).unwrap();
        std::fs::write(
            plug.join("skills/frontend/SKILL.md"),
            "---\nname: frontend-design\ndescription: Distinctive UI\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(plug.join("commands")).unwrap();
        std::fs::write(
            plug.join("commands/polish.md"),
            "---\ndescription: Polish the UI\n---\n",
        )
        .unwrap();
        std::fs::write(
            claude.join("plugins/installed_plugins.json"),
            format!(
                r#"{{"version":2,"plugins":{{"design@m":[{{"installPath":"{}"}}]}}}}"#,
                plug.display()
            ),
        )
        .unwrap();

        let items = discover(&proj, &claude);
        let get = |n: &str| {
            items
                .iter()
                .find(|i| i.name == n)
                .unwrap_or_else(|| panic!("missing {n}"))
        };
        assert_eq!(
            get("deploy").source,
            Source::Project,
            "project shadows user"
        );
        assert_eq!(get("deploy").description, "Ship it");
        assert_eq!(get("commit").description, "Write a good commit message.");
        assert_eq!(get("seo-audit").description, "Audit a site for SEO");
        assert!(get("seo-audit").is_skill);
        assert_eq!(get("design:frontend-design").source, Source::Plugin);
        assert_eq!(get("design:polish").description, "Polish the UI");
        assert_eq!(get("compact").source, Source::BuiltIn);
        assert_eq!(items.iter().filter(|i| i.name == "deploy").count(), 1);

        let f = filter(&items, "fro");
        assert_eq!(
            f[0].name, "design:frontend-design",
            "prefix match on the part after the colon"
        );
        assert!(filter(&items, "seo").iter().any(|i| i.name == "seo-audit"));
        assert!(
            filter(&items, "context window")
                .iter()
                .any(|i| i.name == "context"),
            "matches descriptions"
        );
    }
}
