//! Live usage: plan limits (5-hour / weekly), spend and token burn rates,
//! and today's token totals across every Claude Code session on the machine.

use crate::statusline::{LimitWindow, RateLimits};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Time series of (unix seconds, value), bounded in length.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Series {
    pub points: VecDeque<(f64, f64)>,
    #[serde(skip)]
    cap: usize,
}

impl Series {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            points: VecDeque::new(),
            cap,
        }
    }

    /// Append unless the value is unchanged and the last sample is recent.
    pub fn push(&mut self, t: f64, v: f64) {
        if let Some(&(lt, lv)) = self.points.back() {
            if t < lt {
                return;
            }
            if lv == v && t - lt < 30.0 {
                return;
            }
        }
        self.points.push_back((t, v));
        let cap = if self.cap == 0 { 2000 } else { self.cap };
        while self.points.len() > cap {
            self.points.pop_front();
        }
    }

    pub fn last(&self) -> Option<f64> {
        self.points.back().map(|p| p.1)
    }

    /// Change per hour over the trailing `window_secs`, from a cumulative series.
    pub fn rate_per_hour(&self, now: f64, window_secs: f64) -> Option<f64> {
        let last = *self.points.back()?;
        let first = self.points.iter().find(|p| p.0 >= now - window_secs)?;
        let dt = (last.0 - first.0).max(60.0);
        let dv = last.1 - first.1;
        (dv >= 0.0 && self.points.len() > 1).then(|| dv / dt * 3600.0)
    }

    /// Values within the trailing window, for sparklines.
    pub fn window(&self, now: f64, window_secs: f64) -> Vec<(f64, f64)> {
        self.points
            .iter()
            .copied()
            .filter(|p| p.0 >= now - window_secs)
            .collect()
    }
}

/// Plan limits as last reported by any session, with a history of the
/// 5-hour share so the trend survives restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountUsage {
    pub limits: Option<RateLimits>,
    pub observed_at: Option<i64>,
    pub five_hour_history: Series,
    pub seven_day_history: Series,
}

/// A limit crossed one of the alert thresholds.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitAlert {
    pub window: &'static str,
    pub threshold: u8,
    pub used: f32,
    pub resets_at: i64,
}

pub const ALERT_THRESHOLDS: [u8; 2] = [80, 95];

impl AccountUsage {
    fn path() -> PathBuf {
        crate::paths::data_dir().join("usage.json")
    }

    pub fn load() -> Self {
        let mut u: Self = std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        u.five_hour_history.cap = 4000;
        u.seven_day_history.cap = 4000;
        // Keep a week of history.
        let cutoff = (now_secs() - 7 * 86_400) as f64;
        for s in [&mut u.five_hour_history, &mut u.seven_day_history] {
            while s.points.front().is_some_and(|p| p.0 < cutoff) {
                s.points.pop_front();
            }
        }
        u
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec(self) {
            let p = Self::path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = crate::util::write_private(&p, &b);
        }
    }

    /// Record a fresh report. Returns alerts for thresholds crossed upward
    /// within the same window (a reset clears them).
    pub fn observe(&mut self, r: RateLimits, at: i64) -> Vec<LimitAlert> {
        let mut alerts = vec![];
        let prev = self.limits;
        for (name, new, old) in [
            ("5-hour", r.five_hour, prev.and_then(|p| p.five_hour)),
            ("weekly", r.seven_day, prev.and_then(|p| p.seven_day)),
        ] {
            let Some(new) = new else { continue };
            let before = old
                .filter(|o| o.resets_at == new.resets_at)
                .map(|o| o.used_percentage)
                .unwrap_or(0.0);
            // Highest crossed threshold only: one notification per jump.
            if let Some(t) = ALERT_THRESHOLDS
                .iter()
                .rev()
                .copied()
                .find(|t| before < *t as f32 && new.used_percentage >= *t as f32)
            {
                alerts.push(LimitAlert {
                    window: name,
                    threshold: t,
                    used: new.used_percentage,
                    resets_at: new.resets_at,
                });
            }
        }
        if let Some(w) = r.five_hour {
            self.five_hour_history
                .push(at as f64, w.used_percentage as f64);
        }
        if let Some(w) = r.seven_day {
            self.seven_day_history
                .push(at as f64, w.used_percentage as f64);
        }
        self.limits = Some(r);
        self.observed_at = Some(at);
        alerts
    }

    /// A window whose reset time has passed is reported as fresh (0%).
    pub fn current(&self, now: i64) -> (Option<LimitWindow>, Option<LimitWindow>) {
        let fresh = |w: Option<LimitWindow>| {
            w.map(|w| {
                if w.resets_at <= now {
                    LimitWindow {
                        used_percentage: 0.0,
                        ..w
                    }
                } else {
                    w
                }
            })
        };
        let l = self.limits.unwrap_or_default();
        (fresh(l.five_hour), fresh(l.seven_day))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// Token usage for one local day across all transcripts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyUsage {
    pub day_start: i64,
    pub by_model: BTreeMap<String, Tokens>,
    /// Total tokens per local hour of the day.
    pub by_hour: [u64; 24],
    pub requests: u64,
    pub sessions: u64,
}

impl DailyUsage {
    pub fn total(&self) -> Tokens {
        let mut t = Tokens::default();
        for v in self.by_model.values() {
            t.add(v);
        }
        t
    }
}

/// Seconds since epoch of local midnight today.
pub fn local_day_start(now: i64) -> i64 {
    let dt = chrono::DateTime::from_timestamp(now, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local);
    let midnight = dt.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default();
    midnight
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|d| d.timestamp())
        .unwrap_or(now - now % 86_400)
}

/// Sum today's assistant usage across every transcript modified today.
/// Streaming writes one line per content block with the same usage, so
/// lines are de-duplicated by message id + request id.
pub fn scan_today(projects_dir: &Path, now: i64) -> DailyUsage {
    let day_start = local_day_start(now);
    let mut out = DailyUsage {
        day_start,
        ..Default::default()
    };
    let mut seen: HashSet<String> = HashSet::new();
    let Ok(dirs) = std::fs::read_dir(projects_dir) else {
        return out;
    };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let modified = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if modified < day_start {
                continue;
            }
            if scan_file(&p, day_start, &mut seen, &mut out) {
                out.sessions += 1;
            }
        }
    }
    out
}

fn scan_file(p: &Path, day_start: i64, seen: &mut HashSet<String>, out: &mut DailyUsage) -> bool {
    let Ok(f) = std::fs::File::open(p) else {
        return false;
    };
    let mut any = false;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        // Cheap prefilter before parsing JSON.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v["type"].as_str() != Some("assistant") {
            continue;
        }
        let Some(ts) = v["timestamp"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let ts = ts.timestamp();
        if ts < day_start || ts >= day_start + 86_400 {
            continue;
        }
        let msg = &v["message"];
        let key = format!(
            "{}:{}",
            msg["id"].as_str().unwrap_or(""),
            v["requestId"].as_str().unwrap_or("")
        );
        if key != ":" && !seen.insert(key) {
            continue;
        }
        let u = &msg["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let t = Tokens {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        };
        if t.total() == 0 {
            continue;
        }
        let model = msg["model"].as_str().unwrap_or("unknown");
        if model == "<synthetic>" {
            continue;
        }
        out.by_model.entry(model.to_string()).or_default().add(&t);
        let hour = (((ts - day_start) / 3600).clamp(0, 23)) as usize;
        out.by_hour[hour] += t.total();
        out.requests += 1;
        any = true;
    }
    any
}

/// "2h 14m", "38m", "4d 3h".
pub fn fmt_until(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 86_400 {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3600)
    } else if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// "1.2M", "34.5k", "812".
pub fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.2}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{:.0}k", n as f64 / 1e3),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim(p5: f32, r5: i64) -> RateLimits {
        RateLimits {
            five_hour: Some(LimitWindow {
                used_percentage: p5,
                resets_at: r5,
            }),
            seven_day: Some(LimitWindow {
                used_percentage: 10.0,
                resets_at: 9_999_999_999,
            }),
        }
    }

    #[test]
    fn alerts_fire_once_per_window_and_rearm_after_reset() {
        let mut u = AccountUsage::default();
        assert!(u.observe(lim(50.0, 1000), 1).is_empty());
        let a = u.observe(lim(82.0, 1000), 2);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].threshold, 80);
        assert!(
            u.observe(lim(85.0, 1000), 3).is_empty(),
            "no repeat inside the window"
        );
        let a = u.observe(lim(97.0, 1000), 4);
        assert_eq!(a[0].threshold, 95);
        // New window (different reset time) re-arms.
        assert!(u.observe(lim(5.0, 20_000), 5).is_empty());
        assert_eq!(u.observe(lim(81.0, 20_000), 6)[0].threshold, 80);
        // Jumping past both thresholds reports only the highest.
        let mut v = AccountUsage::default();
        let a = v.observe(lim(99.0, 1000), 1);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].threshold, 95);
    }

    #[test]
    fn expired_window_reads_as_fresh() {
        let mut u = AccountUsage::default();
        u.observe(lim(70.0, 100), 1);
        assert_eq!(u.current(50).0.unwrap().used_percentage, 70.0);
        assert_eq!(u.current(200).0.unwrap().used_percentage, 0.0);
    }

    #[test]
    fn burn_rate() {
        let mut s = Series::with_cap(100);
        s.push(0.0, 0.0);
        s.push(1800.0, 1.0);
        s.push(3600.0, 2.0);
        assert!((s.rate_per_hour(3600.0, 7200.0).unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(s.window(3600.0, 2000.0).len(), 2);
    }

    #[test]
    fn scan_dedupes_streamed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-p");
        std::fs::create_dir_all(&proj).unwrap();
        let now = now_secs();
        let ts = chrono::DateTime::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        let line = |id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"r{id}","timestamp":"{ts}","message":{{"id":"m{id}","model":"claude-opus-5-5","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        let body = [line("1"), line("1"), line("2")].join("\n");
        std::fs::write(proj.join("s.jsonl"), body).unwrap();
        let d = scan_today(dir.path(), now);
        assert_eq!(d.requests, 2);
        assert_eq!(d.total().total(), 230);
        assert_eq!(d.sessions, 1);
        assert_eq!(d.by_hour.iter().sum::<u64>(), 230);
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_until(2 * 3600 + 14 * 60), "2h 14m");
        assert_eq!(fmt_until(4 * 86_400 + 3 * 3600), "4d 3h");
        assert_eq!(fmt_tokens(1_234_567), "1.2M");
        assert_eq!(fmt_tokens(34_500), "34k");
    }
}
