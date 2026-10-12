//! Live load of this PC for `system` sources: CPU, memory, GPU, disk and
//! network as Task Manager's Processes tab shows them, plus the processes
//! using the most of each. Counters come from PDH (added by their English
//! names, so any display language works) and the process list from one
//! `NtQuerySystemInformation` call per sample. Nothing here is cached or
//! written anywhere.

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows_sys::Win32::System::Performance::*;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount};

use crate::usage::{Label, Reading, Window};
use crate::wide;

pub const CPU: &str = "cpu";
pub const MEMORY: &str = "memory";
pub const GPU: &str = "gpu";
pub const DISK: &str = "disk";
pub const NETWORK: &str = "network";
/// Every metric a `system` source can draw, in tooltip order.
pub const METRICS: [&str; 5] = [CPU, MEMORY, GPU, DISK, NETWORK];

/// Tooltip verbs (normal, "remaining" mode) for load and for memory.
const BUSY: (&str, &str) = ("busy", "idle");
const USED: (&str, &str) = ("used", "free");

/// Not in windows-sys: lets utilisation counters report above 100 so the
/// sums below are not capped per process.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// What one sample found for one metric.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metric {
    pub key: &'static str,
    pub window: Option<Window>,
    /// Shown after the percentage, e.g. the busiest GPU engine or disk.
    pub info: Option<String>,
    /// Processes using the most, as display name and amount (percent, bytes or bytes/s).
    pub top: Vec<(String, f64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub metrics: Vec<Metric>,
    pub at: i64,
}

impl Snapshot {
    /// The reading one source shows: its metrics, with up to `top` processes each.
    pub fn reading(&self, keys: &[String], top: usize) -> Reading {
        let mut r = Reading::new(self.at);
        for m in self.metrics.iter().filter(|m| keys.iter().any(|k| k == m.key)) {
            if let Some(w) = m.window {
                r.add(m.key, w);
            }
            let detail = (top > 0 && !m.top.is_empty()).then(|| top_line(m.key, &m.top[..m.top.len().min(top)]));
            r.labels.push(Label {
                key: m.key.to_owned(),
                unit: (m.key == MEMORY).then(|| "GB".to_owned()).or_else(|| (m.key == NETWORK).then(|| "Mbps".to_owned())),
                info: m.info.clone(),
                detail,
                verbs: Some(if m.key == MEMORY { USED } else { BUSY }),
                ..Default::default()
            });
        }
        r.windows.sort_by_key(|(k, _)| METRICS.iter().position(|m| m == k));
        r
    }
}

/// `chrome 8% · Code 5%`, `chrome 3.2 GB · Code 410 MB`, `top I/O: chrome 2.1 MB/s`.
fn top_line(key: &str, top: &[(String, f64)]) -> String {
    let items: Vec<String> = top
        .iter()
        .map(|(name, v)| match key {
            MEMORY => format!("{name} {}", bytes(*v)),
            DISK => format!("{name} {}/s", bytes(*v)),
            _ => format!("{name} {}%", v.round()),
        })
        .collect();
    let list = items.join(" \u{00B7} ");
    if key == DISK { format!("top I/O: {list}") } else { list }
}

pub fn bytes(n: f64) -> String {
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= 1024.0 * 1024.0 {
        let mb = n / (1024.0 * 1024.0);
        if mb >= 100.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") }
    } else {
        format!("{:.0} KB", n / 1024.0)
    }
}

// ---------------------------------------------------------------- PDH

struct Counter(PDH_HCOUNTER);

fn add_counter(query: PDH_HQUERY, path: &str) -> Option<Counter> {
    let mut c: PDH_HCOUNTER = null_mut();
    (unsafe { PdhAddEnglishCounterW(query, wide(path).as_ptr(), 0, &mut c) } == 0).then_some(Counter(c))
}

fn valid(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

impl Counter {
    fn value(&self) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let rc = unsafe { PdhGetFormattedCounterValue(self.0, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, null_mut(), &mut v) };
        (rc == 0 && valid(v.CStatus)).then_some(unsafe { v.Anonymous.doubleValue })
    }

    /// `(instance, value)` for every instance of a wildcard counter.
    fn values(&self, buf: &mut Vec<u64>) -> Vec<(String, f64)> {
        let fmt = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
        let mut size = 0u32;
        let mut count = 0u32;
        let rc = unsafe { PdhGetFormattedCounterArrayW(self.0, fmt, &mut size, &mut count, null_mut()) };
        if rc != PDH_MORE_DATA {
            return Vec::new();
        }
        // Instances can appear between the two calls: leave some room.
        let words = (size as usize + 4096).div_ceil(8);
        if buf.len() < words {
            buf.resize(words, 0);
        }
        size = (buf.len() * 8) as u32;
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if unsafe { PdhGetFormattedCounterArrayW(self.0, fmt, &mut size, &mut count, items) } != 0 {
            return Vec::new();
        }
        let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
        items
            .iter()
            .filter(|i| valid(i.FmtValue.CStatus) && !i.szName.is_null())
            .map(|i| (unsafe { pwstr(i.szName) }, unsafe { i.FmtValue.Anonymous.doubleValue }))
            .collect()
    }
}

unsafe fn pwstr(p: *const u16) -> String {
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

// ---------------------------------------------------------------- processes

/// The documented prefix of `SYSTEM_PROCESS_INFORMATION` with the fields
/// winternl.h leaves "reserved" named as in the Windows SDK's debugger headers.
#[repr(C)]
struct ProcessInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_length: u16,
    image_name_maximum_length: u16,
    image_name_buffer: *const u16,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<ProcessInfo>() == 0x100);

struct Process {
    pid: u32,
    name: String,
    create_time: i64,
    /// User plus kernel time, 100 ns units.
    cpu: i64,
    private_ws: f64,
    /// Bytes read and written, all devices.
    io: i64,
}

/// Every running process. `buf` is kept between calls.
fn processes(buf: &mut Vec<u64>) -> Vec<Process> {
    if buf.is_empty() {
        buf.resize(256 * 1024, 0);
    }
    loop {
        let mut needed = 0u32;
        let len = (buf.len() * 8).min(u32::MAX as usize) as u32;
        let rc = unsafe { NtQuerySystemInformation(SystemProcessInformation, buf.as_mut_ptr().cast(), len, &mut needed) };
        if rc == STATUS_INFO_LENGTH_MISMATCH && buf.len() < 16 * 1024 * 1024 {
            let words = (needed as usize + 64 * 1024).div_ceil(8).max(buf.len() * 2);
            buf.resize(words, 0);
            continue;
        }
        if rc < 0 {
            return Vec::new();
        }
        break;
    }
    let base = buf.as_ptr() as *const u8;
    let end = buf.len() * 8;
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<ProcessInfo>() <= end {
        let p = unsafe { &*(base.add(offset) as *const ProcessInfo) };
        let pid = p.unique_process_id as u32;
        if pid != 0 {
            let name = if p.image_name_buffer.is_null() || p.image_name_length == 0 {
                "System".to_owned()
            } else {
                String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p.image_name_buffer, p.image_name_length as usize / 2) })
            };
            out.push(Process {
                pid,
                name: display_name(&name),
                create_time: p.create_time,
                cpu: p.user_time + p.kernel_time,
                private_ws: p.working_set_private_size as f64,
                io: p.read_transfer_count + p.write_transfer_count,
            });
        }
        if p.next_entry_offset == 0 {
            break;
        }
        offset += p.next_entry_offset as usize;
    }
    out
}

/// `chrome.exe` -> `chrome`.
fn display_name(image: &str) -> String {
    match image.len().checked_sub(4) {
        Some(cut) if image.is_char_boundary(cut) && image[cut..].eq_ignore_ascii_case(".exe") => image[..cut].to_owned(),
        _ => image.to_owned(),
    }
}

/// The `n` largest `(name, amount)` totals. Processes with the same name are
/// added up and counted, as Task Manager groups them: `chrome (18)`.
fn largest(items: impl Iterator<Item = (String, f64)>, n: usize, min: f64) -> Vec<(String, f64)> {
    let mut sums: HashMap<String, (String, f64, usize)> = HashMap::new();
    for (name, v) in items {
        let e = sums.entry(name.to_lowercase()).or_insert_with(|| (name, 0.0, 0));
        e.1 += v;
        e.2 += 1;
    }
    let mut list: Vec<(String, f64)> = sums
        .into_values()
        .filter(|(_, v, _)| *v >= min)
        .map(|(name, v, count)| (if count > 1 { format!("{name} ({count})") } else { name }, v))
        .collect();
    list.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list.truncate(n);
    list
}

// ---------------------------------------------------------------- GPU

/// One `GPU Engine` instance: `pid_1234_luid_0x0_0x1_phys_0_eng_3_engtype_3D`.
#[derive(Debug, PartialEq)]
struct Engine<'a> {
    pid: u32,
    /// Adapter, physical GPU and engine index: the part shared by every process.
    engine: &'a str,
    kind: &'a str,
}

fn parse_engine(instance: &str) -> Option<Engine<'_>> {
    let rest = instance.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let at = rest.find("_engtype_")?;
    Some(Engine { pid: pid.parse().ok()?, engine: &rest[..at], kind: &rest[at + "_engtype_".len()..] })
}

/// GPU load the way Task Manager reports it: each engine's utilisation summed
/// over processes, then the busiest engine. Returns that load, the engine's
/// type, and each process's own busiest engine.
fn gpu_load(items: &[(String, f64)]) -> (f64, String, HashMap<u32, f64>) {
    let mut engines: HashMap<&str, (f64, &str)> = HashMap::new();
    let mut per_pid: HashMap<(u32, &str), f64> = HashMap::new();
    for (name, v) in items {
        let Some(e) = parse_engine(name) else { continue };
        let slot = engines.entry(e.engine).or_insert((0.0, e.kind));
        slot.0 += v;
        *per_pid.entry((e.pid, e.engine)).or_default() += v;
    }
    let (load, kind) = engines.values().fold((0.0, ""), |best, (v, k)| if *v > best.0 { (*v, *k) } else { best });
    let mut pids: HashMap<u32, f64> = HashMap::new();
    for ((pid, _), v) in per_pid {
        let e = pids.entry(pid).or_default();
        *e = e.max(v);
    }
    (load.min(100.0), kind.to_owned(), pids)
}

// ---------------------------------------------------------------- sampler

/// PDH instance names spell brackets for parentheses: `Intel[R] I211`.
fn instance_name(s: &str) -> String {
    s.replace('[', "(").replace(']', ")")
}

/// `0 C:` -> `disk 0 (C:)`.
fn disk_name(instance: &str) -> String {
    match instance.split_once(' ') {
        Some((n, letters)) if n.bytes().all(|b| b.is_ascii_digit()) => format!("disk {n} ({letters})"),
        _ => instance.to_owned(),
    }
}

struct Previous {
    at: Instant,
    /// pid -> (create time, cpu time, io bytes)
    procs: HashMap<u32, (i64, i64, i64)>,
}

/// Measures the metrics it was created for. Keep one alive and call
/// [`Sampler::sample`] on an interval: load figures are averages since the
/// previous call, so the first call only primes the counters.
pub struct Sampler {
    query: PDH_HQUERY,
    cpu: Option<Counter>,
    gpu: Option<Counter>,
    disk_idle: Option<Counter>,
    disk_bytes: Option<Counter>,
    net_bytes: Option<Counter>,
    net_bandwidth: Option<Counter>,
    want: Vec<&'static str>,
    top: usize,
    cores: f64,
    pdh_buf: Vec<u64>,
    proc_buf: Vec<u64>,
    previous: Option<Previous>,
}

// The query handle is only ever used by the thread that owns the sampler.
unsafe impl Send for Sampler {}

impl Sampler {
    /// `metrics` are keys from [`METRICS`]; `top` processes are listed per metric (0 skips the process scan).
    pub fn new(metrics: &[String], top: usize) -> Sampler {
        let want: Vec<&'static str> = METRICS.into_iter().filter(|m| metrics.iter().any(|k| k == m)).collect();
        let mut query: PDH_HQUERY = null_mut();
        if unsafe { PdhOpenQueryW(null(), 0, &mut query) } != 0 {
            query = null_mut();
        }
        let has = |m: &str| want.contains(&m) && !query.is_null();
        let add = |on: bool, path: &str| if on { add_counter(query, path) } else { None };
        Sampler {
            // Task Manager's Processes tab shows % Processor Time (its Performance
            // tab shows % Processor Utility, which counts turbo clocks above 100).
            cpu: add(has(CPU), r"\Processor Information(_Total)\% Processor Time"),
            gpu: add(has(GPU), r"\GPU Engine(*)\Utilization Percentage"),
            disk_idle: add(has(DISK), r"\PhysicalDisk(*)\% Idle Time"),
            disk_bytes: add(has(DISK), r"\PhysicalDisk(_Total)\Disk Bytes/sec"),
            net_bytes: add(has(NETWORK), r"\Network Interface(*)\Bytes Total/sec"),
            net_bandwidth: add(has(NETWORK), r"\Network Interface(*)\Current Bandwidth"),
            query,
            want,
            top,
            cores: f64::from(unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1)),
            pdh_buf: Vec::new(),
            proc_buf: Vec::new(),
            previous: None,
        }
    }

    pub fn sample(&mut self, at: i64) -> Snapshot {
        if !self.query.is_null() {
            unsafe { PdhCollectQueryData(self.query) };
        }
        let now = Instant::now();
        // Per-process rates since the previous sample.
        let scan = self.top > 0 && self.want.iter().any(|m| [CPU, MEMORY, GPU, DISK].contains(m));
        let procs = if scan { processes(&mut self.proc_buf) } else { Vec::new() };
        let mut cpu_share: Vec<(String, f64)> = Vec::new();
        let mut io_rate: Vec<(String, f64)> = Vec::new();
        if let Some(prev) = &self.previous {
            let secs = now.duration_since(prev.at).as_secs_f64();
            if secs > 0.05 {
                for p in &procs {
                    let Some(&(created, cpu, io)) = prev.procs.get(&p.pid) else { continue };
                    if created != p.create_time {
                        continue;
                    }
                    cpu_share.push((p.name.clone(), (p.cpu - cpu).max(0) as f64 / (secs * 1e7 * self.cores) * 100.0));
                    io_rate.push((p.name.clone(), (p.io - io).max(0) as f64 / secs));
                }
            }
        }
        self.previous = Some(Previous { at: now, procs: procs.iter().map(|p| (p.pid, (p.create_time, p.cpu, p.io))).collect() });
        let names: HashMap<u32, &str> = procs.iter().map(|p| (p.pid, p.name.as_str())).collect();
        let metrics = self.want.clone().into_iter().map(|key| self.metric(key, &procs, &names, &cpu_share, &io_rate)).collect();
        Snapshot { metrics, at }
    }

    fn metric(
        &mut self,
        key: &'static str,
        procs: &[Process],
        names: &HashMap<u32, &str>,
        cpu_share: &[(String, f64)],
        io_rate: &[(String, f64)],
    ) -> Metric {
        let top = self.top;
        let mut m = Metric { key, ..Default::default() };
        match key {
            CPU => {
                if let Some(c) = self.cpu.as_ref().and_then(Counter::value) {
                    m.window = Some(Window::pct(c.clamp(0.0, 100.0), 0));
                    m.top = largest(cpu_share.iter().cloned(), top, 0.5);
                }
            }
            MEMORY => {
                let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
                ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
                if unsafe { GlobalMemoryStatusEx(&mut ms) } != 0 && ms.ullTotalPhys > 0 {
                    let total = ms.ullTotalPhys as f64;
                    let used = total - ms.ullAvailPhys as f64;
                    m.window = Some(Window {
                        used: used / total * 100.0,
                        resets_at: 0,
                        value: Some(round1(used / GB)),
                        limit: Some(round1(total / GB)),
                    });
                    m.top = largest(procs.iter().map(|p| (p.name.clone(), p.private_ws)), top, 1024.0 * 1024.0);
                }
            }
            GPU => {
                if let Some(c) = &self.gpu {
                    let items = c.values(&mut self.pdh_buf);
                    // No instances at all (no GPU driver counters): not reported.
                    if !items.is_empty() {
                        let (load, kind, pids) = gpu_load(&items);
                        m.window = Some(Window::pct(load, 0));
                        m.info = (load >= 0.5 && !kind.is_empty()).then(|| format!("{kind} engine"));
                        m.top = largest(pids.into_iter().filter_map(|(pid, v)| Some(((*names.get(&pid)?).to_owned(), v))), top, 0.5);
                    }
                }
            }
            DISK => {
                if let Some(c) = &self.disk_idle {
                    let mut total = None;
                    let mut disks: Vec<(String, f64)> = Vec::new();
                    for (n, idle) in c.values(&mut self.pdh_buf) {
                        let active = (100.0 - idle).clamp(0.0, 100.0);
                        if n == "_Total" { total = Some(active) } else { disks.push((n, active)) }
                    }
                    // Task Manager's figure: active time averaged over the disks.
                    if let Some(active) = total {
                        m.window = Some(Window::pct(active, 0));
                        let rate = format!("{}/s", bytes(self.disk_bytes.as_ref().and_then(Counter::value).unwrap_or(0.0)));
                        let busiest = disks.iter().max_by(|a, b| a.1.total_cmp(&b.1)).filter(|_| disks.len() > 1);
                        m.info = Some(match busiest {
                            Some((name, pct)) => format!("{rate} \u{00B7} busiest {} {}%", disk_name(name), pct.round()),
                            None => rate,
                        });
                        m.top = largest(io_rate.iter().cloned(), top, 1024.0);
                    }
                }
            }
            NETWORK => {
                if let (Some(b), Some(w)) = (&self.net_bytes, &self.net_bandwidth) {
                    let bandwidth: HashMap<String, f64> = w.values(&mut self.pdh_buf).into_iter().collect();
                    let busiest = b
                        .values(&mut self.pdh_buf)
                        .into_iter()
                        .filter_map(|(n, rate)| {
                            let bw = *bandwidth.get(&n).filter(|bw| **bw > 0.0)?;
                            Some((n, rate * 8.0, bw))
                        })
                        .max_by(|a, b| (a.1 / a.2).total_cmp(&(b.1 / b.2)).then(a.2.total_cmp(&b.2)));
                    if let Some((name, bits, bw)) = busiest {
                        m.window = Some(Window {
                            used: (bits / bw * 100.0).clamp(0.0, 100.0),
                            resets_at: 0,
                            value: Some(round1(bits / 1e6)),
                            limit: Some(round1(bw / 1e6)),
                        });
                        m.info = Some(instance_name(&name));
                    }
                }
            }
            _ => {}
        }
        m
    }
}

/// One reading, sampled over a second (for `--probe`; the widget keeps a sampler running).
pub fn measure(metrics: &[String], top: usize) -> Reading {
    let mut s = Sampler::new(metrics, top);
    s.sample(crate::time::now());
    std::thread::sleep(std::time::Duration::from_secs(1));
    s.sample(crate::time::now()).reading(metrics, top)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

impl Drop for Sampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_and_gpu_load() {
        let e = parse_engine("pid_7500_luid_0x00000000_0x00012838_phys_0_eng_0_engtype_3D").unwrap();
        assert_eq!(e, Engine { pid: 7500, engine: "luid_0x00000000_0x00012838_phys_0_eng_0", kind: "3D" });
        assert_eq!(parse_engine("_Total"), None);
        let items: Vec<(String, f64)> = [
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_4_engtype_VideoDecode", 40.0),
            ("pid_3_luid_0x0_0x1_phys_0_eng_1_engtype_Copy", 70.0),
        ]
        .iter()
        .map(|(n, v)| ((*n).to_owned(), *v))
        .collect();
        let (load, kind, pids) = gpu_load(&items);
        // 3D: 30 + 25 = 55 beats VideoDecode 40 but not Copy 70.
        assert_eq!((load, kind.as_str()), (70.0, "Copy"));
        assert_eq!((pids[&1], pids[&2], pids[&3]), (30.0, 40.0, 70.0));
        assert_eq!(gpu_load(&[("pid_1_luid_a_eng_0_engtype_3D".into(), 180.0)]).0, 100.0);
    }

    #[test]
    fn names_and_lists() {
        assert_eq!(display_name("chrome.exe"), "chrome");
        assert_eq!(display_name("Memory Compression"), "Memory Compression");
        assert_eq!(disk_name("1 D:"), "disk 1 (D:)");
        assert_eq!(instance_name("Intel[R] I211 Gigabit Network Connection"), "Intel(R) I211 Gigabit Network Connection");
        let l =
            largest([("chrome", 3.0), ("Code", 4.0), ("Chrome", 2.0), ("idle", 0.2)].iter().map(|(n, v)| ((*n).to_owned(), *v)), 2, 0.5);
        assert_eq!(l, [("chrome (2)".to_owned(), 5.0), ("Code".to_owned(), 4.0)]);
        assert_eq!(bytes(3.2 * GB), "3.2 GB");
        assert_eq!(bytes(410.0 * 1024.0 * 1024.0), "410 MB");
        assert_eq!(bytes(2.15 * 1024.0 * 1024.0), "2.1 MB");
        assert_eq!(bytes(5000.0), "5 KB");
    }

    #[test]
    fn readings_per_source() {
        let snap = Snapshot {
            at: 9,
            metrics: vec![
                Metric { key: GPU, window: Some(Window::pct(14.0, 0)), info: Some("3D engine".into()), top: vec![("dwm".into(), 6.0)] },
                Metric {
                    key: MEMORY,
                    window: Some(Window { used: 61.0, resets_at: 0, value: Some(19.4), limit: Some(31.9) }),
                    info: None,
                    top: vec![("chrome".into(), 3.2 * GB), ("Code".into(), 410.0 * 1024.0 * 1024.0)],
                },
                Metric { key: CPU, window: None, info: None, top: vec![] },
            ],
        };
        let r = snap.reading(&[CPU.into(), MEMORY.into(), GPU.into()], 1);
        let keys: Vec<&str> = r.windows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, [MEMORY, GPU]);
        assert_eq!(r.fetched_at, 9);
        let mem = r.label(MEMORY).unwrap();
        assert_eq!((mem.unit.as_deref(), mem.detail.as_deref(), mem.verbs), (Some("GB"), Some("chrome 3.2 GB"), Some(USED)));
        assert_eq!(r.label(GPU).unwrap().info.as_deref(), Some("3D engine"));
        assert_eq!(r.label(GPU).unwrap().detail.as_deref(), Some("dwm 6%"));
        assert_eq!(snap.reading(&[GPU.into()], 0).label(GPU).unwrap().detail, None);
        assert_eq!(top_line(DISK, &[("rustc".into(), 2.15 * 1024.0 * 1024.0)]), "top I/O: rustc 2.1 MB/s");
    }
}
