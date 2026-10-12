//! `http` provider: one request to any JSON endpoint, with the bars mapped
//! from the response by JSON paths. Secrets come from `vars` and are
//! substituted into the URL, headers and body as `${name}`;
//! `${env:NAME}` reads an environment variable directly.

use std::collections::HashMap;

use serde_json::Value;

use super::values::{jwt_exp, resolve};
use super::{Outcome, expired, limited, now};
use crate::config::{Amount, Expiry, HttpSpec, WindowSpec};
use crate::jpath;
use crate::net::{Http, parse_url};
use crate::usage::{Label, Reading, Window, percent, time_value};

const IDLE: &str = "Sign-in expired. Showing the last values; they refresh once the sign-in does.";

pub fn fetch(http: &Http, spec: &HttpSpec) -> Result<Reading, Outcome> {
    let mut vars = HashMap::new();
    for (name, src) in &spec.vars {
        let v = resolve(src).map_err(|e| Outcome::Failed(format!("{name}: {e}")))?;
        vars.insert(name.as_str(), v);
    }
    if let Some(e) = &spec.expires {
        let at = match e {
            Expiry::Jwt(var) => vars.get(var.as_str()).and_then(|t| jwt_exp(t)),
            Expiry::At(src) => resolve(src).ok().and_then(|s| time_value(&Value::String(s))),
        };
        if expired(at) {
            return Err(Outcome::Idle(IDLE.into()));
        }
    }
    let fill = |s: &str| fill(s, &vars).map_err(Outcome::Failed);
    let url = fill(&spec.url)?;
    let target = parse_url(&url).map_err(Outcome::Failed)?;
    let mut headers: Vec<(String, String)> = Vec::new();
    for (k, v) in &spec.headers {
        headers.push((k.clone(), fill(v)?));
    }
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
        headers.push(("Accept".into(), "application/json".into()));
    }
    let body = match &spec.body {
        Some(b) => {
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("Content-Type".into(), "application/json".into()));
            }
            fill(b)?
        }
        None => String::new(),
    };
    let header_refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let resp = http.request(&spec.method, &target, &header_refs, body.as_bytes()).map_err(Outcome::Failed)?;
    let json: Option<Value> = serde_json::from_slice(&resp.body).ok();
    match resp.status {
        200..=299 => {}
        401 | 403 if spec.expires.is_some() => return Err(Outcome::Idle(IDLE.into())),
        429 => return Err(limited(resp.retry_after)),
        s => {
            let hint = match s {
                401 | 403 => " (check the credentials)",
                404 => " (check the URL)",
                _ => "",
            };
            let msg = json.as_ref().and_then(error_message).map(|m| format!(": {m}")).unwrap_or_default();
            return Err(Outcome::Failed(format!("HTTP {s}{hint}{msg}")));
        }
    }
    let json = json.ok_or_else(|| Outcome::Failed("the response is not JSON".into()))?;
    map(spec, &json, now())
}

/// Replaces `${name}` with a variable and `${env:NAME}` with an environment variable.
fn fill(template: &str, vars: &HashMap<&str, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').ok_or_else(|| "unclosed ${ in the request".to_owned())?;
        let name = &tail[..end];
        let value = match name.strip_prefix("env:") {
            Some(env) => std::env::var(env).map_err(|_| format!("environment variable {env} is not set"))?,
            None => vars.get(name).cloned().ok_or_else(|| format!("unknown variable ${{{name}}} (add it to \"vars\")"))?,
        };
        out.push_str(&value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A short error message from a JSON error body, for the tooltip.
fn error_message(v: &Value) -> Option<String> {
    let m = ["/error/message", "/message", "/error", "/detail", "/error_description"]
        .iter()
        .find_map(|p| jpath::get(v, p).and_then(jpath::text))?;
    Some(m.chars().take(120).collect())
}

fn amount(v: &Value, a: &Option<Amount>) -> Option<f64> {
    match a.as_ref()? {
        Amount::Const(n) => Some(*n),
        Amount::Path(p) => jpath::get(v, p).and_then(jpath::number),
    }
}

fn window(spec: &WindowSpec, v: &Value, now: i64) -> Option<Window> {
    let (used, remaining, limit) = (amount(v, &spec.used), amount(v, &spec.remaining), amount(v, &spec.limit));
    let pct = percent(amount(v, &spec.percent), used, remaining, limit, spec.scale).or(spec.default)?;
    let path = |p: &Option<String>| jpath::get(v, p.as_deref()?);
    let resets_at = path(&spec.resets_at)
        .and_then(time_value)
        .or_else(|| path(&spec.resets_in).and_then(jpath::number).map(|s| now + s as i64))
        .unwrap_or(0);
    let value = used.or_else(|| Some(limit? - remaining?));
    Some(Window { used: pct, resets_at, value, limit })
}

fn map(spec: &HttpSpec, v: &Value, now: i64) -> Result<Reading, Outcome> {
    let mut r = Reading::new(now);
    for w in &spec.windows {
        if let Some(win) = window(w, v, now) {
            r.add(&w.key, win);
        }
        if w.tag.is_some() || w.name.is_some() || w.unit.is_some() {
            r.labels.push(Label {
                key: w.key.clone(),
                tag: w.tag.clone(),
                name: w.name.clone(),
                unit: w.unit.clone(),
                ..Default::default()
            });
        }
    }
    if r.windows.is_empty() {
        return Err(Outcome::Failed("none of the windows were found in the response (check the paths)".into()));
    }
    let text = |p: &Option<String>| jpath::get(v, p.as_deref()?).and_then(jpath::text);
    r.plan = text(&spec.plan);
    r.email = text(&spec.account);
    r.note = text(&spec.note);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn spec(json: &str) -> HttpSpec {
        let v: Value = serde_json::from_str(&format!(r#"{{"sources":[{json}]}}"#)).unwrap();
        let c = config::from_json(Some(&v), std::path::Path::new("Z:/h"), std::path::Path::new("Z:/a"));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        match &c.sources[0].provider {
            config::Provider::Http(h) => (**h).clone(),
            _ => panic!(),
        }
    }

    #[test]
    fn maps_responses() {
        let s = spec(
            r#"{"provider":"http","name":"X","url":"https://x.test/u",
                "windows":[
                  {"key":"credits","tag":"$","used":"/data/total_usage","limit":"/data/total_credits","unit":"USD"},
                  {"key":"daily","percent":"/limits[kind=day]/frac","scale":100,"resets_at":"/limits[kind=day]/reset"},
                  {"key":"rpm","remaining":"/rl/left","limit":60,"resets_in":"/rl/in"},
                  {"key":"proto","percent":"/missing","default":0},
                  {"key":"absent","percent":"/missing"}],
                "plan":"/plan/name","account":"/user","note":"/msg"}"#,
        );
        let v: Value = serde_json::from_str(
            r#"{"data":{"total_usage":"12.5","total_credits":50},
                "limits":[{"kind":"min","frac":0.9},{"kind":"day","frac":0.25,"reset":1791496800000}],
                "rl":{"left":45,"in":30},"plan":{"name":"Pro"},"user":"me@example.com","msg":""}"#,
        )
        .unwrap();
        let r = map(&s, &v, 1000).unwrap();
        assert_eq!(r.get("credits"), Some(Window { used: 25.0, resets_at: 0, value: Some(12.5), limit: Some(50.0) }));
        assert_eq!(r.get("daily"), Some(Window::pct(25.0, 1_791_496_800)));
        assert_eq!(r.get("rpm"), Some(Window { used: 25.0, resets_at: 1030, value: Some(15.0), limit: Some(60.0) }));
        assert_eq!(r.get("proto").unwrap().used, 0.0);
        assert_eq!(r.get("absent"), None);
        assert_eq!(r.label("credits").unwrap().unit.as_deref(), Some("USD"));
        assert_eq!((r.plan.as_deref(), r.email.as_deref(), r.note), (Some("Pro"), Some("me@example.com"), None));
        assert!(matches!(map(&s, &serde_json::json!({"other":1}), 0), Ok(r) if r.windows.len() == 1));
        let strict = spec(r#"{"provider":"http","url":"https://x.test","windows":[{"percent":"/p"}]}"#);
        assert!(matches!(map(&strict, &serde_json::json!({}), 0), Err(Outcome::Failed(_))));
    }

    #[test]
    fn templates() {
        let vars: HashMap<&str, String> = [("key", "abc".to_owned())].into_iter().collect();
        assert_eq!(fill("Bearer ${key}", &vars).unwrap(), "Bearer abc");
        assert_eq!(fill("no vars", &vars).unwrap(), "no vars");
        unsafe { std::env::set_var("LIMITBAR_TEST_ORG", "org-1") };
        assert_eq!(fill("/v1/${env:LIMITBAR_TEST_ORG}/usage?k=${key}", &vars).unwrap(), "/v1/org-1/usage?k=abc");
        assert!(fill("${nope}", &vars).unwrap_err().contains("unknown variable ${nope}"));
        assert!(fill("${key", &vars).is_err());
    }

    #[test]
    fn error_messages() {
        assert_eq!(error_message(&serde_json::json!({"error":{"message":"Invalid API key"}})).as_deref(), Some("Invalid API key"));
        assert_eq!(error_message(&serde_json::json!({"detail":"Not found"})).as_deref(), Some("Not found"));
        assert_eq!(error_message(&serde_json::json!({"error":{"code":1}})), None);
    }
}
