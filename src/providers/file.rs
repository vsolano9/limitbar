//! `file` provider: reads Limitbar's JSON format (see `usage::parse_report`)
//! from a file another tool writes. The file's modification time counts as
//! the time of the reading, so a file nobody updates fades out as stale.

use std::path::Path;
use std::time::UNIX_EPOCH;

use super::{Outcome, from_report, now};
use crate::usage::{Reading, parse_report};

pub fn fetch(path: &Path) -> Result<Reading, Outcome> {
    let text = std::fs::read_to_string(path).map_err(|_| Outcome::Failed(format!("{} not found", path.display())))?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| Outcome::Failed(format!("{} is not valid JSON ({e})", path.display())))?;
    let mut r = from_report(parse_report(&v, now()))?;
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some(secs) = modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64) {
        r.fetched_at = secs.min(r.fetched_at);
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file() {
        let path = std::env::temp_dir().join(format!("limitbar-file-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"windows":[{"key":"monthly","used":30,"limit":120,"unit":"requests"}],"note":"hi"}"#).unwrap();
        let r = fetch(&path).unwrap();
        assert_eq!(r.get("monthly").unwrap().used, 25.0);
        assert_eq!(r.note.as_deref(), Some("hi"));
        assert!(r.fetched_at <= now() && r.fetched_at > now() - 60);
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(fetch(&path), Err(Outcome::Failed(_))));
        std::fs::remove_file(&path).ok();
        assert!(matches!(fetch(&path), Err(Outcome::Failed(_))));
    }
}
