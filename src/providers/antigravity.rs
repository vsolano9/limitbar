//! Antigravity (Google's `agy` CLI, Google AI Pro/Ultra sign-in): two 5-hour
//! pools, Gemini models (`gemini`) and the partner models Antigravity also
//! serves (`partner`: Claude, GPT-OSS).
//!
//! Credentials: read-only from the freshest non-expired sign-in among:
//! - Windows Credential Manager `gemini:antigravity` (standalone `agy`),
//! - `<dir>\antigravity-cli\antigravity-oauth-token` (file-storage `agy`), and
//! - `<T3CODE_HOME|~/.t3>\userdata\providers\antigravity\*\antigravity-acp\antigravity-oauth-token`
//!   (T3 Code ACP profiles, which refresh file storage rather than Credential Manager).
//!
//! Each uses the same JSON (`token.access_token`, `token.expiry`, `id_token_email`).
//! The project comes from `<profile>\cache\default_project_id.txt` or
//! `<dir>\antigravity-cli\cache\default_project_id.txt` when present.
//! Endpoints: `POST daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`
//! (each model's `quotaInfo.remainingFraction` and `resetTime`), plan from
//! `v1internal:loadCodeAssist` (`paidTier.name`). Undocumented: every field is optional.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::values::credential;
use super::{Outcome, Profile, expired, json_body, now};
use crate::net::Http;
use crate::time::parse_iso8601;
use crate::usage::{AG_GEMINI, AG_PARTNER, Reading, Window};

pub const CREDENTIAL: &str = "gemini:antigravity";
const TOKEN_FILE: &str = "antigravity-oauth-token";
const HOST: &str = "daily-cloudcode-pa.googleapis.com";
/// The endpoint serves only clients that name Antigravity in the user agent
/// (others get HTTP 403, "This client is no longer supported"), so Limitbar
/// names itself and the client whose quota it reads.
const USER_AGENT: &str = concat!("Limitbar/", env!("CARGO_PKG_VERSION"), " antigravity");
const IDLE: &str = "Sign-in token expired or revoked. Showing the last values; run agy to refresh it.";

fn t3_token_paths() -> Vec<PathBuf> {
    let t3_home =
        std::env::var_os("T3CODE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::config::home().join(".t3"));
    let root = t3_home.join("userdata").join("providers").join("antigravity");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> =
        entries.flatten().map(|e| e.path().join("antigravity-acp").join(TOKEN_FILE)).filter(|p| p.is_file()).collect();
    out.sort();
    out
}

fn read_project_id(primary_dir: &Path, fallback_dir: &Path) -> Option<String> {
    for base in [primary_dir, fallback_dir] {
        if let Ok(s) = std::fs::read_to_string(base.join("cache").join("default_project_id.txt")) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

struct Candidate {
    token: String,
    email: Option<String>,
    expiry: Option<i64>,
    project_dir: PathBuf,
}

fn collect_candidates(dir: &Path) -> (Vec<Candidate>, bool, bool) {
    let cli_dir = dir.join("antigravity-cli");
    let mut raw_sources: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(raw) = credential(CREDENTIAL) {
        raw_sources.push((raw, cli_dir.clone()));
    }
    let standalone = cli_dir.join(TOKEN_FILE);
    if let Ok(raw) = std::fs::read_to_string(&standalone) {
        raw_sources.push((raw, cli_dir.clone()));
    }
    for path in t3_token_paths() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let pdir = path.parent().map_or_else(|| cli_dir.clone(), Path::to_path_buf);
            raw_sources.push((raw, pdir));
        }
    }
    let mut fresh = Vec::new();
    let mut saw_expired = false;
    let mut saw_bad_json = false;
    for (raw, project_dir) in raw_sources {
        let text = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let Ok(auth) = serde_json::from_str::<Value>(text) else {
            saw_bad_json = true;
            continue;
        };
        let Some(token) = auth["token"]["access_token"].as_str().filter(|t| !t.is_empty()) else {
            continue;
        };
        let expiry = auth["token"]["expiry"].as_str().and_then(parse_iso8601);
        if expired(expiry) {
            saw_expired = true;
            continue;
        }
        fresh.push(Candidate { token: token.to_owned(), email: auth["id_token_email"].as_str().map(str::to_owned), expiry, project_dir });
    }
    // Prefer the token with the latest known expiry so a live T3 or CLI sign-in
    // wins over a stale one.
    fresh.sort_by(|a, b| b.expiry.unwrap_or(i64::MIN).cmp(&a.expiry.unwrap_or(i64::MIN)));
    (fresh, saw_expired, saw_bad_json)
}

/// Whether `agy` (standalone or in a T3 profile) has stored a sign-in on this PC.
pub fn signed_in() -> bool {
    credential(CREDENTIAL).is_ok()
        || crate::config::home().join(".gemini").join("antigravity-cli").join(TOKEN_FILE).is_file()
        || !t3_token_paths().is_empty()
}

pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome> {
    let (candidates, saw_expired, saw_bad_json) = collect_candidates(dir);
    if candidates.is_empty() {
        if saw_expired {
            return Err(Outcome::Idle(IDLE.into()));
        }
        if saw_bad_json {
            return Err(Outcome::Failed("Antigravity sign-in is not JSON".into()));
        }
        return Err(Outcome::Failed("Antigravity is not signed in (run agy)".into()));
    }
    let fallback_dir = dir.join("antigravity-cli");
    let mut last_idle: Option<Outcome> = None;
    for cand in candidates {
        let bearer = format!("Bearer {}", cand.token);
        let headers = [
            ("Authorization", bearer.as_str()),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("User-Agent", USER_AGENT),
        ];
        let call = |method: &str, body: &Value| http.post(HOST, &format!("/v1internal:{method}"), &headers, body.to_string().as_bytes());
        let project = read_project_id(&cand.project_dir, &fallback_dir);
        let body = project.map_or_else(|| serde_json::json!({}), |p| serde_json::json!({ "project": p }));
        let v = match json_body(call("fetchAvailableModels", &body), IDLE) {
            Ok(v) => v,
            Err(Outcome::Idle(msg)) => {
                last_idle = Some(Outcome::Idle(msg));
                continue;
            }
            Err(other) => return Err(other),
        };
        let mut r = parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected quota response from Antigravity".into()))?;
        if !profile.plan_checked
            && let Ok(v) = json_body(call("loadCodeAssist", &serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } })), IDLE)
        {
            profile.plan = plan(&v).or(profile.plan.take());
            profile.plan_checked = true;
        }
        r.email = cand.email.or_else(|| profile.email.clone());
        r.plan = profile.plan.clone();
        return Ok(r);
    }
    Err(last_idle.unwrap_or_else(|| Outcome::Idle(IDLE.into())))
}

/// Models in one pool share a quota, so each pool shows its least remaining
/// fraction and the earliest reset among its models. Models without a reset
/// time (tab completion) have no limit and are skipped.
fn parse(v: &Value, now: i64) -> Option<Reading> {
    let models = v.get("models")?.as_object()?;
    let mut pools: [Option<(f64, i64)>; 2] = [None, None];
    for (id, m) in models {
        let q = &m["quotaInfo"];
        let (Some(left), Some(reset)) = (q["remainingFraction"].as_f64(), q["resetTime"].as_str().and_then(parse_iso8601)) else {
            continue;
        };
        let pool = &mut pools[usize::from(!id.starts_with("gemini"))];
        *pool = Some(match *pool {
            Some((l, r)) => (l.min(left), r.min(reset)),
            None => (left, reset),
        });
    }
    let mut r = Reading::new(now);
    for (key, pool) in [AG_GEMINI, AG_PARTNER].into_iter().zip(pools) {
        if let Some((left, reset)) = pool {
            r.add(key, Window::pct(((1.0 - left) * 100.0).clamp(0.0, 100.0), reset));
        }
    }
    (!r.windows.is_empty()).then_some(r)
}

/// `paidTier.name` ("Google AI Pro", "Google AI Ultra"), else `currentTier.name`.
fn plan(v: &Value) -> Option<String> {
    ["paidTier", "currentTier"].into_iter().find_map(|k| v[k]["name"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_and_plan() {
        let v: Value = serde_json::from_str(
            r#"{"models":{
                "gemini-3-flash":{"quotaInfo":{"remainingFraction":0.75,"resetTime":"1970-01-01T05:00:00Z"}},
                "gemini-2.5-pro":{"quotaInfo":{"remainingFraction":0.5,"resetTime":"1970-01-01T05:00:00Z"}},
                "claude-opus-5-5-high":{"quotaInfo":{"remainingFraction":1,"resetTime":"1970-01-01T05:01:00Z"}},
                "tab_flash_lite_preview":{"quotaInfo":{"remainingFraction":1}}}}"#,
        )
        .unwrap();
        let r = parse(&v, 0).unwrap();
        assert_eq!(r.get(AG_GEMINI), Some(Window::pct(50.0, 18_000)));
        assert_eq!(r.get(AG_PARTNER), Some(Window::pct(0.0, 18_060)));
        assert!(parse(&serde_json::json!({"models":{}}), 0).is_none());
        let p: Value = serde_json::from_str(r#"{"currentTier":{"name":"Antigravity"},"paidTier":{"name":"Google AI Pro"}}"#).unwrap();
        assert_eq!(plan(&p).as_deref(), Some("Google AI Pro"));
    }
}
