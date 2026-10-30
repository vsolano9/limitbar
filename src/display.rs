//! What each source shows, independent of Win32: its rows, their tags and
//! names, and the tooltip lines.

use crate::config::{Bar, MAX_BARS, Source};
use crate::time;
use crate::usage::{self, Reading, Window};

/// One taskbar row of a source, resolved against its latest reading.
pub struct Row<'a> {
    pub key: String,
    pub window: Option<Window>,
    pub bar: Option<&'a Bar>,
}

/// The bars a source draws. Sources without configured bars show the first
/// windows their provider reported, or a single empty row before any data.
/// An optional bar the reading doesn't report is left out.
pub fn rows<'a>(src: &'a Source, reading: Option<&Reading>) -> Vec<Row<'a>> {
    if !src.bars.is_empty() {
        return src
            .bars
            .iter()
            .filter_map(|bar| {
                let found = bar.keys.iter().find_map(|k| Some((k.clone(), reading?.get(k)?)));
                if found.is_none() && bar.optional && reading.is_some() {
                    return None;
                }
                let (key, window) = found.map_or_else(|| (bar.keys[0].clone(), None), |(k, w)| (k, Some(w)));
                Some(Row { key, window, bar: Some(bar) })
            })
            .collect();
    }
    match reading {
        Some(r) if !r.windows.is_empty() => {
            r.windows.iter().take(MAX_BARS).map(|(k, w)| Row { key: k.clone(), window: Some(*w), bar: None }).collect()
        }
        _ => vec![Row { key: String::new(), window: None, bar: None }],
    }
}

/// Every key a source may draw, for a layout that does not shift when data arrives.
pub fn possible_keys(src: &Source, reading: Option<&Reading>) -> Vec<String> {
    let mut keys: Vec<String> = src.bars.iter().flat_map(|b| b.keys.iter().cloned()).collect();
    if keys.is_empty()
        && let Some(r) = reading
    {
        keys = r.windows.iter().take(MAX_BARS).map(|(k, _)| k.clone()).collect();
    }
    keys
}

/// Short tag beside a bar: configured, then reported by the provider, then
/// the well-known tag, then the key itself (up to four characters).
pub fn tag(key: &str, bar: Option<&Bar>, reading: Option<&Reading>) -> String {
    bar.and_then(|b| b.tag.clone())
        .or_else(|| reading.and_then(|r| r.label(key)?.tag.clone()))
        .or_else(|| usage::known_tag(key).map(str::to_owned))
        .unwrap_or_else(|| key.chars().take(4).collect())
}

/// Full window name for the tooltip.
pub fn name(key: &str, bar: Option<&Bar>, reading: Option<&Reading>) -> String {
    bar.and_then(|b| b.name.clone())
        .or_else(|| reading.and_then(|r| r.label(key)?.name.clone()))
        .or_else(|| usage::known_name(key).map(str::to_owned))
        .unwrap_or_else(|| key.to_owned())
}

fn unit(key: &str, bar: Option<&Bar>, reading: Option<&Reading>) -> Option<String> {
    bar.and_then(|b| b.unit.clone()).or_else(|| reading.and_then(|r| r.label(key)?.unit.clone()))
}

/// The percentage shown for a window: used, or left in "remaining" mode.
pub fn shown(w: &Window, now: i64, remaining: bool) -> f64 {
    let used = w.effective(now);
    if remaining { 100.0 - used } else { used }
}

/// One window in the tooltip.
#[derive(Debug)]
pub struct TipEntry {
    pub key: String,
    pub name: String,
    pub window: Option<Window>,
    pub unit: Option<String>,
    pub info: Option<String>,
    pub detail: Option<String>,
    pub verbs: (&'static str, &'static str),
}

fn entry(key: &str, bar: Option<&Bar>, reading: Option<&Reading>, window: Option<Window>) -> TipEntry {
    let label = reading.and_then(|r| r.label(key));
    TipEntry {
        key: key.to_owned(),
        name: name(key, bar, reading),
        window,
        unit: unit(key, bar, reading),
        info: label.and_then(|l| l.info.clone()),
        detail: label.and_then(|l| l.detail.clone()),
        verbs: label.and_then(|l| l.verbs).unwrap_or(("used", "left")),
    }
}

/// Tooltip entries: each drawn row (present or not) plus every other window
/// the provider reported, well-known keys in their usual order.
pub fn tooltip_windows(src: &Source, reading: Option<&Reading>) -> Vec<TipEntry> {
    let drawn = rows(src, reading);
    let mut out: Vec<TipEntry> = drawn.iter().filter(|r| !r.key.is_empty()).map(|r| entry(&r.key, r.bar, reading, r.window)).collect();
    if let Some(r) = reading {
        for (k, w) in &r.windows {
            if !out.iter().any(|o| &o.key == k) {
                let bar = src.bars.iter().find(|b| b.keys.contains(k));
                out.push(entry(k, bar, reading, Some(*w)));
            }
        }
    }
    out.sort_by_key(|e| usage::order(&e.key));
    out
}

fn amount(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e12 {
        format!("{}", n as i64)
    } else if n.abs() >= 100.0 {
        format!("{n:.0}")
    } else {
        format!("{n:.2}").trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// The tooltip line for a usage window. `day_clock` formats a reset time.
pub fn window_line(e: &TipEntry, now: i64, remaining: bool, day_clock: impl Fn(i64, i64) -> String) -> String {
    let name = &e.name;
    let Some(w) = e.window else { return format!("{name}: not reported") };
    let verb = if remaining { e.verbs.1 } else { e.verbs.0 };
    let mut s = format!("{name}: {}% {verb}", shown(&w, now, remaining).round());
    let reset = w.resets_at > 0 && now >= w.resets_at;
    if let (Some(v), Some(l), false) = (w.value, w.limit, reset) {
        let u = e.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
        s.push_str(&format!(" ({} of {}{u})", amount(v), amount(l)));
    }
    if let Some(info) = &e.info {
        s.push_str(&format!(" \u{00B7} {info}"));
    }
    if reset {
        s.push_str(&format!(" \u{00B7} window reset at {}", day_clock(w.resets_at, now)));
    } else if w.resets_at > 0 {
        s.push_str(&format!(" \u{00B7} resets {} (in {})", day_clock(w.resets_at, now), time::countdown(w.resets_at - now)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, Provider};
    use crate::usage::{FIVE_HOUR, Label, WEEKLY};
    use std::path::Path;

    fn source(json: &str) -> Source {
        let v: serde_json::Value = serde_json::from_str(&format!(r#"{{"sources":[{json}]}}"#)).unwrap();
        config::from_json(Some(&v), Path::new("Z:/h"), Path::new("Z:/a")).sources.remove(0)
    }

    fn reading(windows: &[(&str, f64)]) -> Reading {
        let mut r = Reading::new(0);
        for (k, u) in windows {
            r.add(k, Window::pct(*u, 0));
        }
        r
    }

    #[test]
    fn builtin_rows() {
        let codex = source(r#"{"provider":"codex"}"#);
        let r = reading(&[(FIVE_HOUR, 10.0), (WEEKLY, 40.0)]);
        let drawn = rows(&codex, Some(&r));
        assert_eq!(drawn.len(), 1);
        assert_eq!((drawn[0].key.as_str(), drawn[0].window.unwrap().used), (WEEKLY, 40.0));
        // The undrawn 5-hour window is still listed, first.
        let keys: Vec<String> = tooltip_windows(&codex, Some(&r)).into_iter().map(|t| t.key).collect();
        assert_eq!(keys, [FIVE_HOUR, WEEKLY]);
        let grok = source(r#"{"provider":"grok"}"#);
        let drawn = rows(&grok, Some(&reading(&[("monthly", 5.0)])));
        assert_eq!((drawn[0].key.as_str(), tag(&drawn[0].key, drawn[0].bar, None).as_str()), ("monthly", "mo"));
        assert_eq!(rows(&grok, None)[0].key, WEEKLY);
        let cursor = source(r#"{"provider":"cursor"}"#);
        let keys: Vec<String> = tooltip_windows(&cursor, None).into_iter().map(|t| t.key).collect();
        assert_eq!(keys, ["auto", "api"]);
        assert!(matches!(cursor.provider, Provider::Builtin { .. }));
    }

    #[test]
    fn custom_rows_and_labels() {
        let src = source(r#"{"provider":"command","name":"Mine","command":"x"}"#);
        assert_eq!(rows(&src, None).len(), 1);
        assert_eq!(rows(&src, None)[0].key, "");
        assert!(tooltip_windows(&src, None).is_empty());
        let mut r = reading(&[("credits", 25.0), ("a", 1.0), ("b", 2.0), ("c", 3.0)]);
        r.labels.push(Label {
            key: "credits".into(),
            tag: Some("$".into()),
            name: Some("API credits".into()),
            unit: Some("USD".into()),
            ..Default::default()
        });
        let drawn = rows(&src, Some(&r));
        assert_eq!(drawn.len(), MAX_BARS);
        assert_eq!(tag("credits", None, Some(&r)), "$");
        assert_eq!(name("credits", None, Some(&r)), "API credits");
        assert_eq!(tag("requests", None, None), "requ");
        assert_eq!(possible_keys(&src, Some(&r)), ["credits", "a", "b"]);
        let tips = tooltip_windows(&src, Some(&r));
        assert_eq!(tips.len(), 4);
        assert_eq!(tips[0].unit.as_deref(), Some("USD"));
        let with_bars = source(r#"{"provider":"file","path":"x","bars":[{"key":"credits","tag":"cr","name":"Credits"}]}"#);
        let drawn = rows(&with_bars, Some(&r));
        assert_eq!(tag(&drawn[0].key, drawn[0].bar, Some(&r)), "cr");
        assert_eq!(name(&drawn[0].key, drawn[0].bar, Some(&r)), "Credits");
    }

    #[test]
    fn lines() {
        let clock = |_: i64, _: i64| "9:00 PM".to_owned();
        let e = |name: &str, window: Option<Window>, unit: Option<&str>| TipEntry {
            key: String::new(),
            name: name.into(),
            window,
            unit: unit.map(str::to_owned),
            info: None,
            detail: None,
            verbs: ("used", "left"),
        };
        let w = Window { used: 25.0, resets_at: 3700, value: Some(12.5), limit: Some(50.0) };
        assert_eq!(
            window_line(&e("Credits", Some(w), Some("USD")), 100, false, clock),
            "Credits: 25% used (12.5 of 50 USD) \u{00B7} resets 9:00 PM (in 1h 00m)"
        );
        assert_eq!(
            window_line(&e("Credits", Some(w), None), 100, true, clock),
            "Credits: 75% left (12.5 of 50) \u{00B7} resets 9:00 PM (in 1h 00m)"
        );
        assert_eq!(
            window_line(&e("Weekly", Some(Window::pct(80.0, 50)), None), 100, false, clock),
            "Weekly: 0% used \u{00B7} window reset at 9:00 PM"
        );
        assert_eq!(window_line(&e("Daily", Some(Window::pct(33.4, 0)), None), 100, false, clock), "Daily: 33% used");
        assert_eq!(window_line(&e("5-hour", None, None), 100, false, clock), "5-hour: not reported");
        let gpu = TipEntry { info: Some("3D engine".into()), verbs: ("busy", "idle"), ..e("GPU", Some(Window::pct(14.0, 0)), None) };
        assert_eq!(window_line(&gpu, 100, false, clock), "GPU: 14% busy \u{00B7} 3D engine");
        assert_eq!(window_line(&gpu, 100, true, clock), "GPU: 86% idle \u{00B7} 3D engine");
        assert_eq!(amount(1234.567), "1235");
        assert_eq!(amount(3.10), "3.1");
        assert_eq!(shown(&Window::pct(30.0, 0), 1, true), 70.0);
    }
}
