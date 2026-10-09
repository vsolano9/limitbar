//! The usage model shown by the widget, the JSON format that `command` and
//! `file` providers produce, and the state cache.
//!
//! A [`Reading`] holds any number of keyed usage windows. A source decides
//! which of them it draws on the taskbar (see `config::Bar`); the tooltip
//! lists them all.

use serde_json::{Value, json};

use crate::jpath;
use crate::time::parse_iso8601;

pub const FIVE_HOUR: &str = "five_hour";
pub const DAILY: &str = "daily";
pub const WEEKLY: &str = "weekly";
pub const MONTHLY: &str = "monthly";
/// Cursor's own models (Auto + Composer pool).
pub const CURSOR_AUTO: &str = "auto";
/// Named third-party models in Cursor (Claude, GPT, Gemini, ...).
pub const CURSOR_API: &str = "api";

/// Well-known keys: short tag drawn beside the bar and the full name used in
/// the tooltip, in tooltip order. Any other key is shown as reported.
const NAMES: &[(&str, &str, &str)] = &[
    (FIVE_HOUR, "5h", "5-hour"),
    (DAILY, "day", "Daily"),
    (WEEKLY, "wk", "Weekly"),
    (MONTHLY, "mo", "Monthly"),
    (CURSOR_AUTO, "cur", "Cursor models (Auto + Composer)"),
    (CURSOR_API, "api", "Other models (API)"),
];

pub fn known_tag(key: &str) -> Option<&'static str> {
    NAMES.iter().find(|n| n.0 == key).map(|n| n.1)
}

pub fn known_name(key: &str) -> Option<&'static str> {
    NAMES.iter().find(|n| n.0 == key).map(|n| n.2)
}

pub fn order(key: &str) -> usize {
    NAMES.iter().position(|n| n.0 == key).unwrap_or(NAMES.len())
}

/// One limit window: percent used and when it resets (unix seconds, 0 =
/// unknown), plus the raw amounts when the provider reports them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub used: f64,
    pub resets_at: i64,
    pub value: Option<f64>,
    pub limit: Option<f64>,
}

impl Window {
    pub fn pct(used: f64, resets_at: i64) -> Window {
        Window { used, resets_at, value: None, limit: None }
    }

    /// Usage as of `now`: a window whose reset time has passed is back to 0%.
    /// This keeps idle accounts correct without refreshing their tokens.
    pub fn effective(&self, now: i64) -> f64 {
        if self.resets_at > 0 && now >= self.resets_at { 0.0 } else { self.used.clamp(0.0, 100.0) }
    }
}

/// Tag, name and unit a provider reported for one of its keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Label {
    pub key: String,
    pub tag: Option<String>,
    pub name: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Keyed windows, in the order reported.
    pub windows: Vec<(String, Window)>,
    pub labels: Vec<Label>,
    pub fetched_at: i64,
    /// Account (usually an e-mail address) and plan, when known.
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Status line from the provider, e.g. Cursor's "You've hit your usage limit".
    pub note: Option<String>,
}

impl Reading {
    pub fn new(now: i64) -> Reading {
        Reading { fetched_at: now, ..Default::default() }
    }

    pub fn get(&self, key: &str) -> Option<Window> {
        self.windows.iter().find(|(k, _)| k == key).map(|(_, w)| *w)
    }

    pub fn label(&self, key: &str) -> Option<&Label> {
        self.labels.iter().find(|l| l.key == key)
    }

    /// Adds a window unless one with the same key is already present.
    pub fn add(&mut self, key: &str, w: Window) {
        if self.get(key).is_none() {
            self.windows.push((key.to_owned(), w));
        }
    }

    pub fn sort_known(&mut self) {
        self.windows.sort_by_key(|(k, _)| order(k));
    }
}

/// Percent used from whichever amounts a provider gives: a percentage, used
/// and limit, or remaining and limit. `scale` multiplies a percentage given
/// as a fraction (0.42 with scale 100 is 42%).
pub fn percent(pct: Option<f64>, used: Option<f64>, remaining: Option<f64>, limit: Option<f64>, scale: f64) -> Option<f64> {
    if let Some(p) = pct {
        return Some(p * scale);
    }
    let limit = limit.filter(|l| *l > 0.0)?;
    match (used, remaining) {
        (Some(u), _) => Some(u / limit * 100.0),
        (None, Some(r)) => Some((limit - r) / limit * 100.0),
        _ => None,
    }
}

/// A point in time given as ISO 8601 text, unix seconds or unix milliseconds
/// (numbers or numeric strings; values above 10^11 are read as milliseconds).
pub fn time_value(v: &Value) -> Option<i64> {
    if let Some(s) = v.as_str()
        && let Some(t) = parse_iso8601(s.trim())
    {
        return Some(t);
    }
    let n = jpath::number(v)?;
    if n <= 0.0 {
        return None;
    }
    Some(if n > 1e11 { (n / 1000.0) as i64 } else { n as i64 })
}

/// What a `command` or `file` provider reported.
#[derive(Debug, PartialEq)]
pub enum Report {
    Usage(Reading),
    Idle(String),
    Error(String),
}

/// Parses Limitbar's provider output format:
///
/// ```json
/// { "windows": [ { "key": "credits", "tag": "cr", "name": "API credits",
///                  "percent": 42, "used": 21, "limit": 50, "remaining": 29,
///                  "resets_at": "2026-11-01T00:00:00Z", "resets_in": 3600, "unit": "USD" } ],
///   "plan": "Pro", "account": "me@example.com", "note": "anything",
///   "idle": "message", "error": "message" }
/// ```
///
/// Each window needs `percent`, or `used` or `remaining` together with
/// `limit`. Everything else is optional.
pub fn parse_report(v: &Value, now: i64) -> Report {
    let s = |k: &str| v.get(k).and_then(jpath::text);
    if let Some(e) = s("error") {
        return Report::Error(e);
    }
    if let Some(m) = s("idle") {
        return Report::Idle(m);
    }
    let Some(list) = v.get("windows").and_then(Value::as_array) else {
        return Report::Error("output has no \"windows\" list".into());
    };
    let mut r = Reading::new(now);
    for (i, w) in list.iter().enumerate() {
        let n = |k: &str| w.get(k).and_then(jpath::number);
        let key = w.get("key").and_then(jpath::text).unwrap_or_else(|| format!("w{}", i + 1));
        let Some(used) = percent(n("percent"), n("used"), n("remaining"), n("limit"), 1.0) else { continue };
        let resets_at = w.get("resets_at").and_then(time_value).or_else(|| n("resets_in").map(|s| now + s as i64)).unwrap_or(0);
        let value = n("used").or_else(|| Some(n("limit")? - n("remaining")?));
        r.add(&key, Window { used, resets_at, value, limit: n("limit") });
        let text = |k: &str| w.get(k).and_then(jpath::text);
        if text("tag").is_some() || text("name").is_some() || text("unit").is_some() {
            r.labels.push(Label { key: key.clone(), tag: text("tag"), name: text("name"), unit: text("unit") });
        }
    }
    if r.windows.is_empty() {
        return Report::Error("output has no usable windows (each needs percent, or used/remaining with limit)".into());
    }
    r.email = s("account").or_else(|| s("email"));
    r.plan = s("plan");
    r.note = s("note");
    Report::Usage(r)
}

pub fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

// ---- cache (no secrets: percentages, amounts, reset times, account, plan, note) ----

pub fn reading_to_json(r: &Reading) -> Value {
    let windows: Vec<Value> = r
        .windows
        .iter()
        .map(|(k, w)| {
            let mut o = json!({ "key": k, "used": w.used, "resets_at": w.resets_at });
            if let Some(v) = w.value {
                o["value"] = json!(v);
            }
            if let Some(l) = w.limit {
                o["limit"] = json!(l);
            }
            if let Some(l) = r.label(k) {
                for (f, val) in [("tag", &l.tag), ("name", &l.name), ("unit", &l.unit)] {
                    if let Some(val) = val {
                        o[f] = json!(val);
                    }
                }
            }
            o
        })
        .collect();
    json!({
        "windows": windows,
        "fetched_at": r.fetched_at,
        "email": r.email,
        "plan": r.plan,
        "note": r.note,
    })
}

/// Reads the current layout and the 1.0 layout (`five_hour` and `weekly`
/// objects at the top level).
pub fn reading_from_json(v: &Value) -> Option<Reading> {
    let window = |w: &Value| {
        Some(Window {
            used: w.get("used")?.as_f64()?,
            resets_at: w.get("resets_at")?.as_i64()?,
            value: w.get("value").and_then(Value::as_f64),
            limit: w.get("limit").and_then(Value::as_f64),
        })
    };
    let mut labels = Vec::new();
    let windows = match v.get("windows").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(|w| {
                let key = w.get("key")?.as_str()?.to_owned();
                let t = |f: &str| w.get(f).and_then(Value::as_str).map(str::to_owned);
                if t("tag").is_some() || t("name").is_some() || t("unit").is_some() {
                    labels.push(Label { key: key.clone(), tag: t("tag"), name: t("name"), unit: t("unit") });
                }
                Some((key, window(w)?))
            })
            .collect(),
        None => [FIVE_HOUR, WEEKLY].into_iter().filter_map(|k| Some((k.to_owned(), window(v.get(k)?)?))).collect(),
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Reading { windows, labels, fetched_at: v.get("fetched_at")?.as_i64()?, email: s("email"), plan: s("plan"), note: s("note") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn reset_rolls_over_to_zero() {
        let w = Window::pct(85.0, 100);
        assert_eq!(w.effective(99), 85.0);
        assert_eq!(w.effective(100), 0.0);
        assert_eq!(Window::pct(140.0, 0).effective(1), 100.0);
    }

    #[test]
    fn percent_from_amounts() {
        assert_eq!(percent(Some(0.42), None, None, None, 100.0), Some(42.0));
        assert_eq!(percent(None, Some(5.0), None, Some(20.0), 1.0), Some(25.0));
        assert_eq!(percent(None, None, Some(15.0), Some(20.0), 1.0), Some(25.0));
        assert_eq!(percent(None, Some(5.0), None, Some(0.0), 1.0), None);
        assert_eq!(percent(None, Some(5.0), None, None, 1.0), None);
    }

    #[test]
    fn time_values() {
        assert_eq!(time_value(&json!("2026-10-08T22:00:00Z")), Some(1_791_496_800));
        assert_eq!(time_value(&json!(1_791_496_800)), Some(1_791_496_800));
        assert_eq!(time_value(&json!("1791496800000")), Some(1_791_496_800));
        assert_eq!(time_value(&json!(0)), None);
        assert_eq!(time_value(&json!("soon")), None);
    }

    #[test]
    fn report_format() {
        let v = parse(
            r#"{"windows":[
                {"key":"credits","tag":"$","name":"API credits","used":12.5,"limit":50,"unit":"USD","resets_at":"2026-11-01T00:00:00Z"},
                {"key":"daily","percent":80,"resets_in":60},
                {"remaining":3,"limit":4},
                {"key":"bad"}],
                "plan":"Pay as you go","account":"me@example.com","note":"ok"}"#,
        );
        let Report::Usage(r) = parse_report(&v, 1000) else { panic!() };
        assert_eq!(r.windows.len(), 3);
        let c = r.get("credits").unwrap();
        assert_eq!((c.used, c.value, c.limit), (25.0, Some(12.5), Some(50.0)));
        assert_eq!(c.resets_at, 1_793_491_200);
        assert_eq!(r.get(DAILY), Some(Window { used: 80.0, resets_at: 1060, value: None, limit: None }));
        assert_eq!(r.get("w3").unwrap().used, 25.0);
        assert_eq!(r.get("w3").unwrap().value, Some(1.0));
        let l = r.label("credits").unwrap();
        assert_eq!((l.tag.as_deref(), l.name.as_deref(), l.unit.as_deref()), (Some("$"), Some("API credits"), Some("USD")));
        assert_eq!((r.plan.as_deref(), r.email.as_deref(), r.note.as_deref()), (Some("Pay as you go"), Some("me@example.com"), Some("ok")));
        assert_eq!(parse_report(&parse(r#"{"idle":"signed out"}"#), 0), Report::Idle("signed out".into()));
        assert_eq!(parse_report(&parse(r#"{"error":"boom","windows":[]}"#), 0), Report::Error("boom".into()));
        assert!(matches!(parse_report(&parse(r#"{"windows":[{"key":"x"}]}"#), 0), Report::Error(_)));
        assert!(matches!(parse_report(&parse(r#"[]"#), 0), Report::Error(_)));
    }

    #[test]
    fn cache_round_trip() {
        let Report::Usage(mut r) = parse_report(
            &parse(
                r#"{"windows":[{"key":"credits","tag":"$","used":1,"limit":4,"unit":"USD"},{"key":"weekly","percent":9,"resets_at":5}]}"#,
            ),
            7,
        ) else {
            panic!()
        };
        r.plan = Some("Ultra".into());
        r.note = Some("n".into());
        assert_eq!(reading_from_json(&reading_to_json(&r)), Some(r));
    }

    #[test]
    fn reads_1_0_cache_entries() {
        let v = parse(
            r#"{"five_hour":{"used":22.0,"resets_at":1791496800},"weekly":{"used":85.0,"resets_at":1791918000},
                "fetched_at":1791490000,"email":"a@b.c","plan":"Max 20x"}"#,
        );
        let r = reading_from_json(&v).unwrap();
        assert_eq!(r.get(FIVE_HOUR), Some(Window::pct(22.0, 1_791_496_800)));
        assert_eq!(r.get(WEEKLY).unwrap().used, 85.0);
        assert_eq!((r.email.as_deref(), r.plan.as_deref(), r.note), (Some("a@b.c"), Some("Max 20x"), None));
        let codex = parse(r#"{"five_hour":null,"weekly":{"used":81.0,"resets_at":1},"fetched_at":3,"email":null,"plan":"Pro"}"#);
        assert_eq!(reading_from_json(&codex).unwrap().windows, vec![(WEEKLY.to_owned(), Window::pct(81.0, 1))]);
        assert!(reading_from_json(&parse(r#"{"weekly":null}"#)).is_none());
    }

    #[test]
    fn known_keys() {
        assert_eq!((known_tag(CURSOR_AUTO), known_tag(CURSOR_API)), (Some("cur"), Some("api")));
        assert_eq!(known_name(FIVE_HOUR), Some("5-hour"));
        assert_eq!(known_tag("credits"), None);
        assert!(order(FIVE_HOUR) < order(WEEKLY));
    }
}
