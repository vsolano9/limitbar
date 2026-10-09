//! Background polling. Each source is checked on its own interval, all due
//! sources in parallel; results are posted to the UI thread as they arrive.
//! A worker belongs to one settings generation and stops when the settings
//! are reloaded.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{SetEvent, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::config::{Source, state_path};
use crate::net::Http;
use crate::providers::{self, Outcome, Profile};
use crate::time::now;
use crate::usage::{self, Reading};

pub const WM_APP_UPDATE: u32 = WM_APP + 1;
pub const WM_APP_ROUND_DONE: u32 = WM_APP + 2;

/// Forced rounds never start closer together than this, however often the user clicks.
const MIN_GAP: i64 = 10;

pub struct Update {
    pub generation: u64,
    pub idx: usize,
    pub outcome: Outcome,
}

/// The UI's handle on a running worker.
pub struct Worker {
    pub generation: u64,
    wake: isize,
    current: Arc<AtomicU64>,
}

impl Worker {
    /// Requests an immediate check of every source.
    pub fn refresh(&self) {
        unsafe { SetEvent(self.wake as _) };
    }

    /// Lets the worker finish its current round and exit.
    pub fn stop(&self) {
        self.current.store(0, Ordering::SeqCst);
        self.refresh();
    }
}

pub fn load_cache() -> HashMap<String, Reading> {
    let Ok(bytes) = std::fs::read(state_path()) else { return HashMap::new() };
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&bytes) else { return HashMap::new() };
    map.iter().filter_map(|(k, v)| Some((k.clone(), usage::reading_from_json(v)?))).collect()
}

/// Writes this worker's readings, keeping cache entries of sources it does not
/// know (so hiding a source for a while does not lose its last values).
fn save_cache(sources: &[Source], readings: &[Option<Reading>]) {
    let path = state_path();
    let mut map = match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    for (s, r) in sources.iter().zip(readings) {
        if let Some(r) = r {
            map.insert(s.id.clone(), usage::reading_to_json(r));
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, Value::Object(map).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn post(hwnd: isize, msg: u32, update: Option<Update>) {
    let lparam = update.map_or(0, |u| Box::into_raw(Box::new(u)) as isize);
    let ok = unsafe { PostMessageW(hwnd as _, msg, 0, lparam) };
    if ok == 0 && lparam != 0 {
        drop(unsafe { Box::from_raw(lparam as *mut Update) });
    }
}

/// Starts a worker for `sources` (with their cached readings) that posts to `hwnd`.
pub fn spawn(generation: u64, sources: Vec<Source>, cached: Vec<Option<Reading>>, hwnd: isize) -> Worker {
    let wake = unsafe { windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) } as isize;
    let current = Arc::new(AtomicU64::new(generation));
    let worker = Worker { generation, wake, current: current.clone() };
    std::thread::spawn(move || {
        if let Some(http) = Http::new(providers::user_agent()) {
            run(&http, generation, &sources, cached, hwnd, wake, &current);
        }
        unsafe { CloseHandle(wake as _) };
    });
    worker
}

fn run(http: &Http, generation: u64, sources: &[Source], cached: Vec<Option<Reading>>, hwnd: isize, wake: isize, current: &AtomicU64) {
    let alive = || current.load(Ordering::SeqCst) == generation;
    let mut readings = cached;
    let mut profiles: Vec<Profile> = readings.iter().map(|r| Profile::from_cache(r.as_ref())).collect();
    let mut next_due = vec![0i64; sources.len()];
    let mut blocked_until = vec![0i64; sources.len()];
    let mut last_forced = i64::MIN / 2;
    let mut forced = true;
    while alive() {
        if forced {
            let gap = now() - last_forced;
            if gap < MIN_GAP {
                std::thread::sleep(std::time::Duration::from_secs((MIN_GAP - gap) as u64));
                if !alive() {
                    return;
                }
            }
            last_forced = now();
        }
        let start = now();
        let due: Vec<usize> = (0..sources.len()).filter(|&i| forced || next_due[i] <= start).collect();
        let results: Vec<(usize, Outcome)> = std::thread::scope(|scope| {
            let handles: Vec<_> = profiles
                .iter_mut()
                .enumerate()
                .filter(|(i, _)| due.contains(i))
                .map(|(idx, profile)| {
                    let src = &sources[idx];
                    let blocked = blocked_until[idx];
                    scope.spawn(move || {
                        let outcome = if blocked > now() { Outcome::Limited(blocked) } else { providers::fetch(http, src, profile) };
                        // Post as soon as it is known; the UI keeps its own copy.
                        if alive() {
                            post(hwnd, WM_APP_UPDATE, Some(Update { generation, idx, outcome: outcome.clone() }));
                        }
                        (idx, outcome)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let mut changed = false;
        for (idx, outcome) in results {
            next_due[idx] = start + sources[idx].poll_seconds as i64;
            match outcome {
                Outcome::Fresh(r) => {
                    readings[idx] = Some(r);
                    changed = true;
                }
                Outcome::Limited(until) => {
                    blocked_until[idx] = until;
                    next_due[idx] = next_due[idx].max(until);
                }
                _ => {}
            }
        }
        if changed {
            save_cache(sources, &readings);
        }
        if !alive() {
            return;
        }
        if forced || !due.is_empty() {
            post(hwnd, WM_APP_ROUND_DONE, Some(Update { generation, idx: 0, outcome: Outcome::Idle(String::new()) }));
        }
        let wait = next_due.iter().min().map_or(3600, |t| (t - now()).clamp(1, 3600));
        forced = unsafe { WaitForSingleObject(wake as _, (wait * 1000) as u32) } == WAIT_OBJECT_0;
    }
}
