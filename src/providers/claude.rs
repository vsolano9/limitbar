//! Claude (Claude Code subscription sign-in): the weekly window, plus the
//! 5-hour window on plans that still have one (Ultra drops it, 2026-10-30).
//!
//! Credentials: `<config dir>\.credentials.json` (`claudeAiOauth.accessToken`,
//! `expiresAt` in ms). Endpoint: `GET api.anthropic.com/api/oauth/usage`.

use std::path::Path;

use serde_json::Value;

use super::{Outcome, Profile, expired, json_body, now, read_json};
use crate::net::Http;
use crate::time::parse_iso8601;
use crate::usage::{FIVE_HOUR, Reading, WEEKLY, Window, title_case};

const IDLE: &str = "Sign-in token expired (account idle). Showing the last values; resets still apply.";

pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome> {
    let cred = read_json(&dir.join(".credentials.json")).map_err(Outcome::Failed)?;
    let oauth = &cred["claudeAiOauth"];
    let Some(token) = oauth["accessToken"].as_str() else {
        return Err(Outcome::Failed("not signed in to a Claude subscription".into()));
    };
    // Expired tokens are left for Claude Code to refresh: refreshing here would
    // rotate the refresh token underneath the CLI.
    if expired(oauth["expiresAt"].as_i64().map(|ms| ms / 1000)) {
        return Err(Outcome::Idle(IDLE.into()));
    }
    if profile.email.is_none() {
        profile.email =
            read_json(&dir.join(".claude.json")).ok().and_then(|v| v["oauthAccount"]["emailAddress"].as_str().map(str::to_owned));
    }
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("anthropic-beta", "oauth-2025-04-20"), ("Accept", "application/json")];
    let v = json_body(http.get("api.anthropic.com", "/api/oauth/usage", &headers), IDLE)?;
    let mut r = parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected usage response from Anthropic".into()))?;
    r.email = profile.email.clone();
    r.plan = plan(oauth["rateLimitTier"].as_str(), oauth["subscriptionType"].as_str());
    Ok(r)
}

fn window(v: &Value) -> Option<Window> {
    let used = v.get("utilization")?.as_f64()?;
    let resets_at = v.get("resets_at").and_then(Value::as_str).and_then(parse_iso8601).unwrap_or(0);
    Some(Window::pct(used, resets_at))
}

/// `five_hour` and `seven_day`, with the `limits[]` array (`session`,
/// `weekly_all`) as a fallback shape.
fn parse(v: &Value, now: i64) -> Option<Reading> {
    let mut r = Reading::new(now);
    if let Some(w) = v.get("five_hour").and_then(window) {
        r.add(FIVE_HOUR, w);
    }
    if let Some(w) = v.get("seven_day").and_then(window) {
        r.add(WEEKLY, w);
    }
    if let Some(limits) = v.get("limits").and_then(Value::as_array) {
        for l in limits {
            let Some(used) = l.get("percent").and_then(Value::as_f64) else { continue };
            let resets_at = l.get("resets_at").and_then(Value::as_str).and_then(parse_iso8601).unwrap_or(0);
            match l.get("kind").and_then(Value::as_str) {
                Some("session") => r.add(FIVE_HOUR, Window::pct(used, resets_at)),
                Some("weekly_all") => r.add(WEEKLY, Window::pct(used, resets_at)),
                _ => {}
            }
        }
    }
    r.sort_known();
    (!r.windows.is_empty()).then_some(r)
}

/// `default_claude_max_20x` -> `Ultra` (Anthropic renamed Max 20x to Ultra and
/// Max 5x to Max on 2026-10-30); falls back to the subscription type.
fn plan(rate_limit_tier: Option<&str>, subscription: Option<&str>) -> Option<String> {
    if let Some(t) = rate_limit_tier {
        for (key, name) in [("ultra", "Ultra"), ("max_20x", "Ultra"), ("max_5x", "Max")] {
            if t.contains(key) {
                return Some(name.into());
            }
        }
    }
    subscription.filter(|s| !s.is_empty()).map(title_case)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn usage() {
        let v = json(
            r#"{"five_hour":{"utilization":20.0,"resets_at":"2026-10-08T22:00:00.089159+00:00"},
            "seven_day":{"utilization":85.0,"resets_at":"2026-10-13T19:00:00.089180+00:00"},
            "seven_day_opus":null,
            "limits":[{"kind":"session","percent":20,"resets_at":"2026-10-08T22:00:00+00:00"}]}"#,
        );
        let r = parse(&v, 5).unwrap();
        assert_eq!(r.get(FIVE_HOUR), Some(Window::pct(20.0, 1_791_496_800)));
        assert_eq!(r.get(WEEKLY).unwrap().used, 85.0);
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.fetched_at, 5);
    }

    #[test]
    fn limits_fallback() {
        let v = json(
            r#"{"five_hour":null,"seven_day":null,"limits":[
                {"kind":"session","percent":22,"resets_at":"2026-10-08T22:50:00Z"},
                {"kind":"weekly_all","percent":6,"resets_at":"2026-10-14T10:00:00Z"},
                {"kind":"weekly_scoped","percent":50,"resets_at":"2026-10-14T10:00:00Z"}]}"#,
        );
        let r = parse(&v, 0).unwrap();
        assert_eq!(r.get(FIVE_HOUR).unwrap().used, 22.0);
        assert_eq!(r.get(WEEKLY).unwrap().used, 6.0);
        assert!(parse(&json(r#"{"error":"x"}"#), 0).is_none());
    }

    #[test]
    fn plans() {
        assert_eq!(plan(Some("default_claude_max_20x"), Some("max")).as_deref(), Some("Ultra"));
        assert_eq!(plan(Some("default_claude_max_5x"), Some("max")).as_deref(), Some("Max"));
        assert_eq!(plan(None, Some("ultra")).as_deref(), Some("Ultra"));
        assert_eq!(plan(None, Some("pro")).as_deref(), Some("Pro"));
        assert_eq!(plan(None, None), None);
    }
}
