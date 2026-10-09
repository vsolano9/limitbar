//! Usage providers. Each built-in provider is one module with a `fetch`
//! function that reads the CLI's stored sign-in (never refreshing or writing
//! it), asks the provider's usage endpoint and returns a [`Reading`]. The
//! generic `http`, `command` and `file` providers cover everything else from
//! the settings file. See CONTRIBUTING.md for adding a built-in provider.

mod claude;
mod codex;
mod command;
mod cursor;
mod file;
mod grok;
mod http;
pub mod values;

use std::path::Path;

use serde_json::Value;

use crate::config::{Builtin, Provider, Source};
use crate::net::{Http, Response};
use crate::time::now;
use crate::usage::{Reading, Report};

const RATE_LIMIT_BACKOFF: i64 = 300;
/// A token this close to expiry is treated as expired.
const EXPIRY_MARGIN: i64 = 30;

#[derive(Clone, Debug)]
pub enum Outcome {
    Fresh(Reading),
    /// The stored sign-in has expired: nobody is using the account, so the
    /// cached values (with reset roll-over) are still right.
    Idle(String),
    Failed(String),
    /// Throttled by the provider until this unix time.
    Limited(i64),
}

/// Account details a source keeps between rounds. Seeded from the cache;
/// providers that need a second request for the plan make it once per run.
#[derive(Default)]
pub struct Profile {
    pub email: Option<String>,
    pub plan: Option<String>,
    plan_checked: bool,
}

impl Profile {
    pub fn from_cache(r: Option<&Reading>) -> Profile {
        Profile { email: r.and_then(|r| r.email.clone()), plan: r.and_then(|r| r.plan.clone()), plan_checked: false }
    }
}

/// Fetches one source once. `profile` carries what earlier rounds learned.
pub fn fetch(http: &Http, src: &Source, profile: &mut Profile) -> Outcome {
    let result = match &src.provider {
        Provider::Builtin { kind, dir } => match kind {
            Builtin::Claude => claude::fetch(http, dir, profile),
            Builtin::Codex => codex::fetch(http, dir),
            Builtin::Grok => grok::fetch(http, dir, profile),
            Builtin::Cursor => cursor::fetch(http, dir, profile),
        },
        Provider::Http(spec) => http::fetch(http, spec),
        Provider::Command { line, timeout } => command::fetch(line, *timeout),
        Provider::File { path } => file::fetch(path),
    };
    result.map_or_else(|o| o, Outcome::Fresh)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|_| format!("{} not found", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|_| format!("{} is not valid JSON", path.display()))
}

fn expired(t: Option<i64>) -> bool {
    t.is_some_and(|t| t <= now() + EXPIRY_MARGIN)
}

fn limited(retry_after: Option<u32>) -> Outcome {
    Outcome::Limited(now() + i64::from(retry_after.unwrap_or(0)).max(RATE_LIMIT_BACKOFF))
}

fn status_outcome(status: u32, retry_after: Option<u32>, idle_msg: &str) -> Outcome {
    match status {
        401 | 403 => Outcome::Idle(idle_msg.into()),
        429 => limited(retry_after),
        s => Outcome::Failed(format!("usage request returned HTTP {s}")),
    }
}

/// A 200 response's JSON body, or the outcome to report instead.
fn json_body(resp: Result<Response, String>, idle_msg: &str) -> Result<Value, Outcome> {
    let resp = resp.map_err(Outcome::Failed)?;
    if resp.status != 200 {
        return Err(status_outcome(resp.status, resp.retry_after, idle_msg));
    }
    serde_json::from_slice(&resp.body).map_err(|_| Outcome::Failed("usage response is not JSON".into()))
}

fn from_report(r: Report) -> Result<Reading, Outcome> {
    match r {
        Report::Usage(r) => Ok(r),
        Report::Idle(m) => Err(Outcome::Idle(m)),
        Report::Error(m) => Err(Outcome::Failed(m)),
    }
}

pub fn user_agent() -> &'static str {
    concat!("Limitbar/", env!("CARGO_PKG_VERSION"))
}
