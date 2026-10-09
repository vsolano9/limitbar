//! Paths into JSON responses: RFC 6901 JSON Pointers (`/data/usage`,
//! `/items/0/percent`) plus one extension for arrays of objects,
//! `name[field=value]`, which picks the first element of the array `name`
//! whose `field` equals `value` (`/limits[kind=session]/percent`). A bare
//! `[field=value]` segment filters the current array. The empty path is the
//! whole document.

use serde_json::Value;

pub fn get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    let rest = path.strip_prefix('/')?;
    let mut cur = root;
    for raw in rest.split('/') {
        let (name, filter) = match raw.find('[') {
            Some(i) if raw.ends_with(']') => (&raw[..i], Some(&raw[i + 1..raw.len() - 1])),
            _ => (raw, None),
        };
        let name = unescape(name);
        if !(name.is_empty() && filter.is_some()) {
            cur = match cur {
                Value::Object(map) => map.get(&name)?,
                Value::Array(list) => list.get(name.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        if let Some(f) = filter {
            let (field, want) = f.split_once('=')?;
            cur = cur.as_array()?.iter().find(|item| item.get(field).is_some_and(|v| matches(v, want)))?;
        }
    }
    Some(cur)
}

fn unescape(s: &str) -> String {
    s.replace("~1", "/").replace("~0", "~")
}

fn matches(v: &Value, want: &str) -> bool {
    match v {
        Value::String(s) => s == want,
        Value::Number(n) => n.to_string() == want || n.as_f64().zip(want.parse::<f64>().ok()).is_some_and(|(a, b)| a == b),
        Value::Bool(b) => want == if *b { "true" } else { "false" },
        _ => false,
    }
}

/// A number, or a string holding one (protobuf JSON sends int64 as strings).
pub fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// Text from a string, number or bool (empty strings count as missing).
pub fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pointers_and_filters() {
        let v = json!({
            "data": {"usage": 12.5, "a/b": 1, "list": [10, 20]},
            "limits": [{"kind": "session", "percent": 22}, {"kind": "weekly", "percent": 6, "id": 7}]
        });
        assert_eq!(get(&v, "/data/usage"), Some(&json!(12.5)));
        assert_eq!(get(&v, "/data/a~1b"), Some(&json!(1)));
        assert_eq!(get(&v, "/data/list/1"), Some(&json!(20)));
        assert_eq!(get(&v, "/limits[kind=weekly]/percent"), Some(&json!(6)));
        assert_eq!(get(&v, "/limits/[id=7]/kind"), Some(&json!("weekly")));
        assert_eq!(get(&v, "/limits[kind=monthly]/percent"), None);
        assert_eq!(get(&v, "/nope"), None);
        assert_eq!(get(&v, "data"), None);
        assert_eq!(get(&v, ""), Some(&v));
    }

    #[test]
    fn numbers_and_text() {
        assert_eq!(number(&json!("1792081007000")), Some(1_792_081_007_000.0));
        assert_eq!(number(&json!(3)), Some(3.0));
        assert_eq!(number(&json!("x")), None);
        assert_eq!(text(&json!(" Pro ")), Some("Pro".into()));
        assert_eq!(text(&json!("")), None);
    }
}
