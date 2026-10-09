//! Cursor (Cursor Agent CLI): monthly usage in two pools, Cursor models
//! (`auto`: Auto + Composer) and other models (`api`: named third-party models).
//!
//! Credentials: `%APPDATA%\Cursor\auth.json` (`accessToken`, a JWT); the account
//! e-mail is the only field read from `~\.cursor\cli-config.json`. Endpoints:
//! `POST api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage`,
//! plan from `GetPlanInfo` (Connect JSON). Undocumented: every field is optional.

use std::path::Path;

use serde_json::Value;

use super::values::jwt_exp;
use super::{Outcome, Profile, expired, json_body, now, read_json};
use crate::config::home;
use crate::jpath;
use crate::net::Http;
use crate::usage::{CURSOR_API, CURSOR_AUTO, Reading, Window};

const HOST: &str = "api2.cursor.sh";
const DASHBOARD: &str = "/aiserver.v1.DashboardService/";
const IDLE: &str = "Sign-in token expired or revoked. Showing the last values; run cursor-agent to refresh it.";

pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome> {
    let auth = read_json(&dir.join("auth.json")).map_err(Outcome::Failed)?;
    let Some(token) = auth["accessToken"].as_str().filter(|t| !t.is_empty()) else {
        return Err(Outcome::Failed("Cursor is not signed in (run cursor-agent login)".into()));
    };
    if expired(jwt_exp(token)) {
        return Err(Outcome::Idle(IDLE.into()));
    }
    if profile.email.is_none() {
        profile.email = read_json(&home().join(".cursor").join("cli-config.json"))
            .ok()
            .and_then(|v| v["authInfo"]["email"].as_str().map(str::to_owned));
    }
    let bearer = format!("Bearer {token}");
    let headers = [
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/json"),
        ("Connect-Protocol-Version", "1"),
        ("Accept", "application/json"),
    ];
    let call = |method: &str| http.post(HOST, &format!("{DASHBOARD}{method}"), &headers, b"{}");
    let v = json_body(call("GetCurrentPeriodUsage"), IDLE)?;
    let mut r = parse(&v, now()).ok_or_else(|| Outcome::Failed("unexpected usage response from Cursor".into()))?;
    if !profile.plan_checked
        && let Ok(v) = json_body(call("GetPlanInfo"), IDLE)
    {
        profile.plan = plan(&v).or(profile.plan.take());
        profile.plan_checked = true;
    }
    r.email = profile.email.clone();
    r.plan = profile.plan.clone();
    Ok(r)
}

/// `planUsage.autoPercentUsed` and `apiPercentUsed`, both resetting at
/// `billingCycleEnd` (epoch ms, as a number or string). Zero values are omitted.
fn parse(v: &Value, now: i64) -> Option<Reading> {
    let pu = v.get("planUsage").filter(|p| p.is_object())?;
    let resets_at = v.get("billingCycleEnd").and_then(jpath::number).map_or(0, |ms| (ms / 1000.0) as i64);
    let pct = |k: &str| pu.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let mut r = Reading::new(now);
    r.add(CURSOR_AUTO, Window::pct(pct("autoPercentUsed"), resets_at));
    r.add(CURSOR_API, Window::pct(pct("apiPercentUsed"), resets_at));
    r.note = v.get("displayMessage").and_then(jpath::text);
    Some(r)
}

/// `GetPlanInfo` -> `planInfo.planName`.
fn plan(v: &Value) -> Option<String> {
    v.get("planInfo")?.get("planName").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
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
            r#"{"billingCycleStart":"1789489007000","billingCycleEnd":"1792081007000",
            "planUsage":{"totalSpend":265121,"includedSpend":40000,"limit":40000,"autoPercentUsed":85.01666666666667,
            "apiPercentUsed":100,"totalPercentUsed":85.52290322580646},"enabled":true,
            "displayMessage":"You've hit your usage limit"}"#,
        );
        let r = parse(&v, 0).unwrap();
        let auto = r.get(CURSOR_AUTO).unwrap();
        assert!((auto.used - 85.0167).abs() < 1e-3);
        assert_eq!(auto.resets_at, 1_792_081_007);
        assert_eq!(r.get(CURSOR_API), Some(Window::pct(100.0, 1_792_081_007)));
        assert_eq!(r.note.as_deref(), Some("You've hit your usage limit"));
        // Numeric int64 and omitted zero percentages.
        let r = parse(&json(r#"{"billingCycleEnd":1000,"planUsage":{"apiPercentUsed":3}}"#), 0).unwrap();
        assert_eq!(r.get(CURSOR_AUTO), Some(Window::pct(0.0, 1)));
        assert_eq!(r.note, None);
        assert!(parse(&json(r#"{"code":"unauthenticated"}"#), 0).is_none());
    }

    #[test]
    fn plan_name() {
        assert_eq!(plan(&json(r#"{"planInfo":{"planName":"Ultra","price":"$200/mo"}}"#)).as_deref(), Some("Ultra"));
        assert_eq!(plan(&json(r#"{}"#)), None);
    }
}
