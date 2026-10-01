//! Parse the JSON Claude Code feeds to status line commands. Fields are read
//! defensively; anything missing falls back to transcript-derived values.

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusInfo {
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
    pub context_percent: Option<f32>,
    pub context_window: Option<u64>,
    pub version: Option<String>,
    pub session_id: Option<String>,
    /// Cumulative tokens for the session.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub api_duration_ms: Option<u64>,
    /// Account-wide plan limits (Claude subscription), when the CLI reports them.
    pub rate_limits: Option<RateLimits>,
}

/// One plan window: share used and when it resets (unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LimitWindow {
    pub used_percentage: f32,
    pub resets_at: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<LimitWindow>,
    pub seven_day: Option<LimitWindow>,
}

impl RateLimits {
    fn parse(v: &Value) -> Option<Self> {
        let window = |w: &Value| {
            Some(LimitWindow {
                used_percentage: w["used_percentage"].as_f64()? as f32,
                resets_at: w["resets_at"].as_i64()?,
            })
        };
        let r = Self {
            five_hour: window(&v["five_hour"]),
            seven_day: window(&v["seven_day"]),
        };
        (r.five_hour.is_some() || r.seven_day.is_some()).then_some(r)
    }
}

impl StatusInfo {
    pub fn parse(v: &Value) -> Self {
        let model = v["model"]["display_name"]
            .as_str()
            .or_else(|| v["model"]["id"].as_str())
            .map(str::to_owned);
        let cw = &v["context_window"];
        let context_window = cw["context_window_size"]
            .as_u64()
            .or_else(|| cw["size"].as_u64());
        let context_percent = cw["used_percentage"]
            .as_f64()
            .map(|p| p as f32)
            .or_else(|| {
                let u = &cw["current_usage"];
                let used = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                ]
                .iter()
                .filter_map(|k| u[*k].as_u64())
                .sum::<u64>();
                match (used, context_window) {
                    (u, Some(w)) if u > 0 && w > 0 => Some(u as f32 / w as f32 * 100.0),
                    _ => None,
                }
            });
        Self {
            cost_usd: v["cost"]["total_cost_usd"].as_f64(),
            model,
            context_percent,
            context_window,
            version: v["version"].as_str().map(str::to_owned),
            input_tokens: cw["total_input_tokens"].as_u64(),
            output_tokens: cw["total_output_tokens"].as_u64(),
            lines_added: v["cost"]["total_lines_added"].as_u64(),
            lines_removed: v["cost"]["total_lines_removed"].as_u64(),
            api_duration_ms: v["cost"]["total_api_duration_ms"].as_u64(),
            rate_limits: RateLimits::parse(&v["rate_limits"]),
            session_id: v["session_id"].as_str().map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_known_shapes() {
        let s = StatusInfo::parse(&json!({
            "session_id": "abc", "version": "2.1.286",
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "cost": {"total_cost_usd": 1.25},
            "context_window": {"context_window_size": 200000,
                "current_usage": {"input_tokens": 10, "cache_read_input_tokens": 49990}}
        }));
        assert_eq!(s.cost_usd, Some(1.25));
        assert_eq!(s.model.as_deref(), Some("Opus 5.5"));
        assert_eq!(s.context_percent, Some(25.0));
        let empty = StatusInfo::parse(&json!({}));
        assert!(empty.rate_limits.is_none());
        assert_eq!(empty, StatusInfo::default());
    }

    #[test]
    fn parses_live_2_1_286_payload() {
        // Captured from Claude Code 2.1.286 (paths trimmed).
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/statusline-2.1.286.json"))
                .unwrap();
        let s = StatusInfo::parse(&v);
        assert_eq!(s.context_window, Some(1_000_000));
        assert_eq!(s.cost_usd, Some(0.0));
        assert_eq!(s.input_tokens, Some(0));
        let r = s.rate_limits.unwrap();
        assert_eq!(r.five_hour.unwrap().used_percentage, 10.0);
        assert_eq!(r.seven_day.unwrap().resets_at, 1791183600);
    }
}
