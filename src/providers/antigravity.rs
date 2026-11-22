//! Antigravity (Google's `agy` CLI, Google AI Pro/Ultra sign-in): two 5-hour
//! pools, Gemini models (`gemini`) and the partner models Antigravity also
//! serves (`partner`: Claude, GPT-OSS).
//!
//! Credentials: the generic credential `gemini:antigravity` in Windows
//! Credential Manager (`token.access_token`, `token.expiry`,
//! `id_token_email`), written by `agy`; the project from
//! `<dir>\antigravity-cli\cache\default_project_id.txt` when present.
//! Endpoints: `POST daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`
//! (each model's `quotaInfo.remainingFraction` and `resetTime`), plan from
//! `v1internal:loadCodeAssist` (`paidTier.name`). Undocumented: every field is optional.

use std::path::Path;

use serde_json::Value;

use super::values::credential;
use super::{Outcome, Profile, expired, json_body, now};
use crate::net::Http;
use crate::time::parse_iso8601;
use crate::usage::{AG_GEMINI, AG_PARTNER, Reading, Window};

pub const CREDENTIAL: &str = "gemini:antigravity";
const HOST: &str = "daily-cloudcode-pa.googleapis.com";
/// The endpoint serves only clients that name Antigravity in the user agent
/// (others get HTTP 403, "This client is no longer supported"), so Limitbar
/// names itself and the client whose quota it reads.
const USER_AGENT: &str = concat!("Limitbar/", env!("CARGO_PKG_VERSION"), " antigravity");
const IDLE: &str = "Sign-in token expired or revoked. Showing the last values; run agy to refresh it.";

/// Whether `agy` has stored a sign-in on this PC.
pub fn signed_in() -> bool {
    credential(CREDENTIAL).is_ok()
}

pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome> {
    let raw = credential(CREDENTIAL).map_err(|_| Outcome::Failed("Antigravity is not signed in (run agy)".into()))?;
    let auth: Value = serde_json::from_str(&raw).map_err(|_| Outcome::Failed("Antigravity sign-in is not JSON".into()))?;
    let Some(token) = auth["token"]["access_token"].as_str().filter(|t| !t.is_empty()) else {
        return Err(Outcome::Failed("Antigravity is not signed in (run agy)".into()));
    };
    if expired(auth["token"]["expiry"].as_str().and_then(parse_iso8601)) {
        return Err(Outcome::Idle(IDLE.into()));
    }
    let bearer = format!("Bearer {token}");
    let headers = [
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/json"),
        ("Accept", "application/json"),
        ("User-Agent", USER_AGENT),
    ];
    let call = |method: &str, body: &Value| http.post(HOST, &format!("/v1internal:{method}"), &headers, body.to_string().as_bytes());
    let project = std::fs::read_to_string(dir.join("antigravity-cli").join("cache").join("default_project_id.txt"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let body = project.map_or_else(|| serde_json::json!({}), |p| serde_json::json!({ "project": p }));
    let v = json_body(call("fetchAvailableModels", &body), IDLE)?;
    let mut r = parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected quota response from Antigravity".into()))?;
    if !profile.plan_checked
        && let Ok(v) = json_body(call("loadCodeAssist", &serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } })), IDLE)
    {
        profile.plan = plan(&v).or(profile.plan.take());
        profile.plan_checked = true;
    }
    r.email = auth["id_token_email"].as_str().map(str::to_owned).or_else(|| profile.email.clone());
    r.plan = profile.plan.clone();
    Ok(r)
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
