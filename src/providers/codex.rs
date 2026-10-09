//! Codex (ChatGPT sign-in): weekly window, plus a 5-hour window on plans that have one.
//!
//! Credentials: `%CODEX_HOME%` or `~\.codex\auth.json` (`tokens.access_token`,
//! `tokens.account_id`). Endpoint: `GET chatgpt.com/backend-api/wham/usage`.

use std::path::Path;

use serde_json::Value;

use super::{Outcome, json_body, now, read_json};
use crate::net::Http;
use crate::usage::{FIVE_HOUR, Reading, WEEKLY, Window, title_case};

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const IDLE: &str = "Sign-in token expired or revoked. Showing the last values; run codex to refresh it.";

pub fn fetch(http: &Http, dir: &Path) -> Result<Reading, Outcome> {
    let auth = read_json(&dir.join("auth.json")).map_err(Outcome::Failed)?;
    let (Some(token), Some(account)) = (auth["tokens"]["access_token"].as_str(), auth["tokens"]["account_id"].as_str()) else {
        return Err(Outcome::Failed("Codex is not signed in with ChatGPT (API-key logins have no plan limits)".into()));
    };
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("ChatGPT-Account-Id", account), ("Accept", "application/json")];
    let v = json_body(http.get("chatgpt.com", "/backend-api/wham/usage", &headers), IDLE)?;
    parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected usage response from ChatGPT".into()))
}

/// `rate_limit.primary_window` and `secondary_window`, classified by their
/// length (weekly vs 5-hour).
fn parse(v: &Value, now: i64) -> Option<Reading> {
    let rl = v.get("rate_limit")?;
    let mut r = Reading::new(now);
    for key in ["primary_window", "secondary_window"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(used) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let resets_at = w
            .get("reset_at")
            .and_then(Value::as_i64)
            .or_else(|| w.get("reset_after_seconds").and_then(Value::as_i64).map(|s| now + s))
            .unwrap_or(0);
        let len = w.get("limit_window_seconds").and_then(Value::as_i64).unwrap_or(0);
        let window = Window::pct(used, resets_at);
        if len > 0 && len <= 6 * HOUR {
            r.add(FIVE_HOUR, window);
        } else if len >= DAY || len == 0 {
            r.add(WEEKLY, window);
        }
    }
    if r.windows.is_empty() {
        return None;
    }
    r.sort_known();
    r.email = v.get("email").and_then(Value::as_str).map(str::to_owned);
    r.plan = v.get("plan_type").and_then(Value::as_str).map(title_case);
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn weekly() {
        let v = json(
            r#"{"email":"a@b.c","plan_type":"pro","rate_limit":{"allowed":true,
            "primary_window":{"used_percent":81,"limit_window_seconds":604800,"reset_after_seconds":455814,"reset_at":1791949897},
            "secondary_window":null}}"#,
        );
        let r = parse(&v, 0).unwrap();
        assert_eq!(r.get(WEEKLY), Some(Window::pct(81.0, 1_791_949_897)));
        assert_eq!(r.get(FIVE_HOUR), None);
        assert_eq!(r.plan.as_deref(), Some("Pro"));
        assert_eq!(r.email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn five_hour_and_weekly() {
        let v = json(
            r#"{"rate_limit":{"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_after_seconds":100},
                "secondary_window":{"used_percent":40,"limit_window_seconds":604800,"reset_at":99}}}"#,
        );
        let r = parse(&v, 1000).unwrap();
        assert_eq!(r.get(FIVE_HOUR), Some(Window::pct(12.5, 1100)));
        assert_eq!(r.get(WEEKLY), Some(Window::pct(40.0, 99)));
        let keys: Vec<&str> = r.windows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, [FIVE_HOUR, WEEKLY]);
        assert!(parse(&json(r#"{"rate_limit":{}}"#), 0).is_none());
    }
}
