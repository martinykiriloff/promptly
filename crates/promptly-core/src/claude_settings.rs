//! Claude Code's own settings for an account: permission rules (read and
//! edit), MCP servers and skills (read). Edits keep every other key and the
//! file's key order, and the first edit saves a backup beside it.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// `permissions.allow / ask / deny` from a settings.json.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
}

pub fn settings_path(claude_dir: &Path) -> PathBuf {
    claude_dir.join("settings.json")
}

fn read_json(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| Value::Object(Map::new()))
}

pub fn read_rules(path: &Path) -> Rules {
    let v = read_json(path);
    let list = |k: &str| -> Vec<String> {
        v["permissions"][k]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    };
    Rules {
        allow: list("allow"),
        ask: list("ask"),
        deny: list("deny"),
    }
}

/// Write the rules back, leaving everything else in the file as it was.
pub fn write_rules(path: &Path, rules: &Rules) -> Result<(), String> {
    let mut v = read_json(path);
    let obj = v
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    let perms = obj
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("\"permissions\" is not an object")?;
    for (k, list) in [
        ("allow", &rules.allow),
        ("ask", &rules.ask),
        ("deny", &rules.deny),
    ] {
        if list.is_empty() {
            perms.remove(k);
        } else {
            perms.insert(
                k.into(),
                Value::Array(list.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }
    }
    let backup = path.with_extension("json.promptly-backup");
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("couldn't back up settings: {e}"))?;
    }
    let mut body = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    body.push('\n');
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// Claude Code warns when a Bash rule has a `*` before the rest of the
/// command (it also matches options inserted there). Same check.
pub fn rule_warning(rule: &str) -> Option<String> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(')')?;
    let star = inner.find('*')?;
    let after = inner[star + 1..].trim();
    (!after.is_empty() && !inner.ends_with(":*")).then(|| {
        "A * before the end also matches options inserted there; put * only at the end".into()
    })
}

/// MCP servers configured for the account: (name, how it runs).
pub fn mcp_servers(claude_json: &Path, settings: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    for v in [read_json(claude_json), read_json(settings)] {
        if let Some(m) = v["mcpServers"].as_object() {
            for (name, cfg) in m {
                let how = cfg["command"]
                    .as_str()
                    .map(|c| {
                        let args: Vec<&str> = cfg["args"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|a| a.as_str())
                            .collect();
                        format!("{c} {}", args.join(" ")).trim().to_string()
                    })
                    .or_else(|| cfg["url"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                if !out.iter().any(|(n, _)| n == name) {
                    out.push((name.clone(), how));
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_other_keys_order_and_back_up() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let original = r#"{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(git status)", "Read"],
    "defaultMode": "acceptEdits"
  },
  "hooks": {}
}"#;
        std::fs::write(&p, original).unwrap();
        let mut r = read_rules(&p);
        assert_eq!(r.allow, ["Bash(git status)", "Read"]);
        r.allow.retain(|x| x != "Read");
        r.deny.push("Bash(rm -rf:*)".into());
        write_rules(&p, &r).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["defaultMode"], "acceptEdits");
        assert_eq!(read_rules(&p).deny, ["Bash(rm -rf:*)"]);
        // Key order is kept (model before permissions before hooks).
        let (a, b, c) = (
            text.find("\"model\"").unwrap(),
            text.find("\"permissions\"").unwrap(),
            text.find("\"hooks\"").unwrap(),
        );
        assert!(a < b && b < c, "{text}");
        assert_eq!(
            std::fs::read_to_string(p.with_extension("json.promptly-backup")).unwrap(),
            original
        );
    }

    #[test]
    fn wildcard_warnings_and_mcp() {
        assert!(rule_warning("Bash(grep -o * ./file)").is_some());
        assert_eq!(rule_warning("Bash(npm run test:*)"), None);
        assert_eq!(rule_warning("Bash(git status)"), None);
        assert_eq!(rule_warning("Read"), None);
        let d = tempfile::tempdir().unwrap();
        let cj = d.path().join(".claude.json");
        std::fs::write(
            &cj,
            r#"{"mcpServers":{"linear":{"url":"https://mcp.linear.app"},"fs":{"command":"npx","args":["-y","server-fs"]}}}"#,
        )
        .unwrap();
        assert_eq!(
            mcp_servers(&cj, &d.path().join("none.json")),
            [
                ("fs".to_string(), "npx -y server-fs".to_string()),
                ("linear".to_string(), "https://mcp.linear.app".to_string())
            ]
        );
    }
}
