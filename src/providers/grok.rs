//! Grok (Grok Build CLI): the credit pool of the current period (weekly on
//! SuperGrok plans; the server decides the period).
//!
//! Credentials: `~\.grok\auth.json`, the first `https://auth.x.ai::<id>` entry
//! (`key`, `email`, `expires_at`; a six-hour token only the CLI refreshes).
//! Endpoints: `GET cli-chat-proxy.grok.com/v1/billing?format=credits`, plan
//! from `/v1/settings`. Undocumented: every field is optional.

use std::path::Path;

use serde_json::Value;

use super::{Outcome, Profile, expired, json_body, now, read_json};
use crate::net::Http;
use crate::time::parse_iso8601;
use crate::usage::{DAILY, MONTHLY, Reading, WEEKLY, Window};

const HOST: &str = "cli-chat-proxy.grok.com";
const DAY: i64 = 86_400;
const IDLE: &str = "Sign-in token expired (account idle). Showing the last values; run grok to refresh it.";

struct Auth<'a> {
    token: &'a str,
    email: Option<&'a str>,
    expires_at: Option<i64>,
}

/// `auth.json` maps `https://auth.x.ai::<id>` to the session; the first such
/// entry with a bearer `key` is used.
fn auth(v: &Value) -> Option<Auth<'_>> {
    let map = v.as_object()?;
    map.iter().filter(|(k, _)| k.starts_with("https://auth.x.ai")).find_map(|(_, s)| {
        let token = s.get("key")?.as_str().filter(|t| !t.is_empty())?;
        Some(Auth {
            token,
            email: s.get("email").and_then(Value::as_str).filter(|e| !e.is_empty()),
            // A timestamp without an offset is UTC, as the CLI writes it.
            expires_at: s.get("expires_at").and_then(Value::as_str).and_then(parse_iso8601),
        })
    })
}

pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome> {
    let file = read_json(&dir.join("auth.json")).map_err(Outcome::Failed)?;
    let Some(a) = auth(&file) else {
        return Err(Outcome::Failed("Grok is not signed in (run grok to sign in)".into()));
    };
    if expired(a.expires_at) {
        return Err(Outcome::Idle(IDLE.into()));
    }
    if let Some(e) = a.email {
        profile.email = Some(e.to_owned());
    }
    let bearer = format!("Bearer {}", a.token);
    let headers = [("Authorization", bearer.as_str()), ("Accept", "application/json")];
    let v = json_body(http.get(HOST, "/v1/billing?format=credits", &headers), IDLE)?;
    let mut r = parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected usage response from Grok".into()))?;
    if !profile.plan_checked
        && let Ok(v) = json_body(http.get(HOST, "/v1/settings", &headers), IDLE)
    {
        profile.plan = plan(&v).or(profile.plan.take());
        profile.plan_checked = true;
    }
    r.email = profile.email.clone();
    r.plan = profile.plan.clone();
    Ok(r)
}

/// One credit pool, `config.creditUsagePercent` of the current period
/// (omitted when 0).
fn parse(v: &Value, now: i64) -> Option<Reading> {
    let cfg = v.get("config").filter(|c| c.is_object())?;
    let period = cfg.get("currentPeriod").filter(|p| p.is_object());
    let used = cfg.get("creditUsagePercent").and_then(Value::as_f64);
    if period.is_none() && used.is_none() {
        return None;
    }
    let time = |p: Option<&Value>, k: &str| p.and_then(|p| p.get(k)).and_then(Value::as_str).and_then(parse_iso8601);
    let start = time(period, "start").or_else(|| time(Some(cfg), "billingPeriodStart"));
    let end = time(period, "end").or_else(|| time(Some(cfg), "billingPeriodEnd"));
    let key = match period.and_then(|p| p.get("type")).and_then(Value::as_str) {
        Some("USAGE_PERIOD_TYPE_DAILY") => DAILY,
        Some("USAGE_PERIOD_TYPE_WEEKLY") => WEEKLY,
        Some("USAGE_PERIOD_TYPE_MONTHLY") => MONTHLY,
        // Unknown type: classify by the period's length.
        _ => match (start, end) {
            (Some(s), Some(e)) if e - s <= 2 * DAY => DAILY,
            (Some(s), Some(e)) if e - s > 8 * DAY => MONTHLY,
            _ => WEEKLY,
        },
    };
    let mut r = Reading::new(now);
    r.add(key, Window::pct(used.unwrap_or(0.0), end.unwrap_or(0)));
    Some(r)
}

/// `/v1/settings` -> `subscription_tier_display`.
fn plan(v: &Value) -> Option<String> {
    v.get("subscription_tier_display").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn usage() {
        // Shape captured from the live endpoint on 2026-10-09 (trimmed).
        let v = json(
            r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY",
            "start":"2026-10-06T07:07:27.291497+00:00","end":"2026-10-13T07:07:27.291497+00:00"},
            "creditUsagePercent":4.0,"onDemandCap":{"val":0},
            "productUsage":[{"product":"GrokBuild","usagePercent":4.0},{"product":"GrokChat"},{"product":"GrokImagine"}],
            "billingPeriodStart":"2026-10-06T07:07:27.291497+00:00","billingPeriodEnd":"2026-10-13T07:07:27.291497+00:00"}}"#,
        );
        let r = parse(&v, 9).unwrap();
        // 2026-10-13T07:07:27Z
        assert_eq!(r.windows, vec![(WEEKLY.to_owned(), Window::pct(4.0, 1_791_875_247))]);
        assert_eq!(r.fetched_at, 9);
    }

    #[test]
    fn defaults_and_periods() {
        // creditUsagePercent omitted (proto3 zero), monthly period.
        let v = json(
            r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_MONTHLY","start":"2026-10-01T00:00:00Z","end":"2026-11-01T00:00:00Z"}}}"#,
        );
        assert_eq!(parse(&v, 0).unwrap().get(MONTHLY).unwrap().used, 0.0);
        // Unknown type: classified by length; end falls back to billingPeriodEnd.
        let v = json(
            r#"{"config":{"currentPeriod":{"type":"X","start":"2026-10-08T00:00:00Z"},"creditUsagePercent":50,
                "billingPeriodEnd":"2026-10-09T00:00:00Z"}}"#,
        );
        assert_eq!(parse(&v, 0).unwrap().get(DAILY).unwrap().resets_at, 1_791_504_000);
        assert!(parse(&json(r#"{"config":{}}"#), 0).is_none());
        assert!(parse(&json(r#"{"error":"unauthorized"}"#), 0).is_none());
    }

    #[test]
    fn auth_file() {
        let v = json(
            r#"{"other::x":{"key":"nope"},
                "https://auth.x.ai::0b1c":{"key":"tok","auth_mode":"oidc","email":"me@x.ai",
                "refresh_token":"r","expires_at":"2026-10-09T13:59:46.751881200Z"}}"#,
        );
        let a = auth(&v).unwrap();
        assert_eq!((a.token, a.email), ("tok", Some("me@x.ai")));
        // 2026-10-09T13:59:46Z
        assert_eq!(a.expires_at, Some(1_791_554_386));
        // No offset is read as UTC; an explicit offset is honoured.
        let at = |s: &str| {
            let v = serde_json::json!({ "https://auth.x.ai::1": { "key": "k", "expires_at": s } });
            auth(&v).unwrap().expires_at
        };
        assert_eq!(at("2026-10-09T13:59:46.75188"), Some(1_791_554_386));
        assert_eq!(at("2026-10-09T15:59:46+02:00"), Some(1_791_554_386));
        assert_eq!(at("soon"), None);
        assert!(auth(&serde_json::json!({ "https://auth.x.ai::1": { "key": "" } })).is_none());
        assert!(auth(&serde_json::json!([])).is_none());
    }

    #[test]
    fn plan_name() {
        assert_eq!(plan(&json(r#"{"subscription_tier_display":"SuperGrok Heavy","x":1}"#)).as_deref(), Some("SuperGrok Heavy"));
        assert_eq!(plan(&json(r#"{"subscription_tier_display":""}"#)), None);
    }
}
