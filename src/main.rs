//! Limitbar: a tiny widget docked inside the Windows 11 taskbar that shows
//! how much of your AI plan limits (or any other quota) you have used, and
//! optionally this PC's CPU, memory, GPU, disk and network load.
//!
//! The widget is a per-pixel-alpha layered child window of `Shell_TrayWnd`, so
//! it follows the taskbar through auto-hide, full-screen apps and moves. A
//! hidden top-level host window owns the state, timers, tooltip and menu,
//! re-attaches the widget when Explorer restarts and reloads the settings
//! file when it changes.

#![windows_subsystem = "windows"]
// Provider results are `Result<Reading, Outcome>`: the error path is taken once
// per poll at most, so boxing it would only add noise.
#![allow(clippy::result_large_err)]

mod config;
mod display;
mod jpath;
mod jsonc;
mod net;
mod providers;
mod render;
mod sys;
mod system;
mod time;
mod usage;
mod worker;

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use config::{Config, Palette, Position, Provider, Source, ThemeMode};
use providers::{Outcome, Profile};
use render::{Align, Font, Painter, Rgba, Surface};
use usage::Reading;
use worker::{Polled, Update, WM_APP_ROUND_DONE, WM_APP_UPDATE, Worker};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const HOST_CLASS: &str = "Limitbar.Host";
const WIDGET_CLASS: &str = "Limitbar.Widget";
const WM_APP_REFRESH: u32 = WM_APP + 3;
const TIMER_WATCH: usize = 1;
const TIMER_TICK: usize = 2;
const CMD_REFRESH: usize = 1;
const CMD_SETTINGS: usize = 3;
const CMD_STARTUP: usize = 4;
const CMD_EXIT: usize = 5;
/// "Open ... usage page" commands are this plus the page's index.
const CMD_PAGE: usize = 100;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

enum Status {
    Waiting,
    Live,
    Idle(String),
    Error(String),
    Limited(i64),
}

struct View {
    src: Source,
    reading: Option<Reading>,
    status: Status,
}

struct Fonts {
    key: (u32, String, u32),
    label: Font,
    tag: Font,
    pct: Font,
}

/// Geometry in physical pixels for the current DPI.
struct Metrics {
    scale: f32,
    pad: i32,
    labels: Vec<i32>,
    /// Width of each block's row-tag column (`5h`/`wk`, `cur`/`api`, ...).
    tags: Vec<i32>,
    bar: i32,
    pct: i32,
    gap_label: i32,
    gap_tag: i32,
    gap_bar: i32,
    gap_block: i32,
}

impl Metrics {
    fn block_width(&self, i: usize) -> i32 {
        let tag = if self.tags[i] > 0 { self.tags[i] + self.gap_tag } else { 0 };
        let pct = if self.pct > 0 { self.gap_bar + self.pct } else { 0 };
        self.labels[i] + self.gap_label + tag + self.bar + pct
    }

    /// `(x, width)` of each block inside the widget.
    fn blocks(&self) -> Vec<(i32, i32)> {
        let mut x = self.pad;
        (0..self.labels.len())
            .map(|i| {
                let w = self.block_width(i);
                let b = (x, w);
                x += w + self.gap_block;
                b
            })
            .collect()
    }

    fn width(&self) -> i32 {
        self.blocks().last().map_or(self.pad * 2, |(x, w)| x + w + self.pad)
    }
}

struct App {
    hinst: HINSTANCE,
    host: HWND,
    widget: HWND,
    tip: HWND,
    taskbar: HWND,
    cfg: Config,
    config_stamp: Option<std::time::SystemTime>,
    views: Vec<View>,
    worker: Worker,
    next_generation: u64,
    refreshing: bool,
    light: bool,
    /// `(x, y, w, h, dpi)` last applied to the widget, in taskbar client coordinates.
    placed: Option<(i32, i32, i32, i32, u32)>,
    fonts: Option<Fonts>,
    measure: Surface,
    painter: Option<Painter>,
    /// Tooltip text buffers; the tooltip keeps pointers into them.
    tips: Vec<Vec<u16>>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state unless it is already borrowed further up the
/// stack (a re-entrant message); callers never hold the borrow across modal loops.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

/// Shown when nothing is configured or signed in, so the widget never vanishes.
fn placeholder() -> Source {
    Source {
        id: String::new(),
        label: "LB".into(),
        name: "Limitbar".into(),
        provider: Provider::File { path: PathBuf::new() },
        bars: Vec::new(),
        color: config::ColorPair { dark: config::DARK.dim, light: config::LIGHT.dim },
        url: None,
        poll_seconds: 0,
    }
}

const NO_SOURCES: &str = "No signed-in providers found. Sign in to Claude Code, Codex, Grok Build or Cursor Agent, \
or add your own provider: right-click > Edit settings.";

fn build_views(cfg: &Config, previous: Vec<View>) -> Vec<View> {
    if cfg.sources.is_empty() {
        return vec![View { src: placeholder(), reading: None, status: Status::Error(NO_SOURCES.into()) }];
    }
    let mut cache = worker::load_cache();
    for v in previous {
        if let Some(r) = v.reading {
            cache.insert(v.src.id, r);
        }
    }
    cfg.sources.iter().map(|s| View { src: s.clone(), reading: cache.get(&s.id).cloned(), status: Status::Waiting }).collect()
}

/// Polls every real source (not the placeholder) of `views`.
fn spawn_worker(views: &[View], generation: u64, host: HWND) -> Worker {
    let real = views.iter().enumerate().filter(|(_, v)| !v.src.id.is_empty());
    let (system, polled): (Vec<_>, Vec<_>) = real.partition(|(_, v)| v.src.is_system());
    let polled = polled.into_iter().map(|(idx, v)| Polled { idx, src: v.src.clone(), cached: v.reading.clone() }).collect();
    let system = system.into_iter().map(|(idx, v)| (idx, v.src.clone())).collect();
    worker::spawn(generation, polled, system, host as isize)
}

/// Seconds after which a reading is drawn faded as out of date.
fn stale_after(src: &Source) -> i64 {
    if src.is_system() { (src.poll_seconds as i64 * 5).max(10) } else { (src.poll_seconds as i64 * 3).max(600) }
}

fn severity(used: f64, accent: Rgba, cfg: &Config, p: &Palette) -> (Rgba, Rgba) {
    if used >= cfg.appearance.critical_at {
        (p.critical, p.critical)
    } else if used >= cfg.appearance.warn_at {
        (p.warn, p.warn)
    } else {
        (accent, p.text)
    }
}

/// Vertical offsets (logical pixels from the centre line) of `n` stacked rows.
fn row_offsets(n: usize) -> impl Iterator<Item = f32> {
    let pitch: f32 = if n >= 3 { 13.0 } else { 16.0 };
    (0..n).map(move |k| (k as f32 - (n as f32 - 1.0) / 2.0) * pitch)
}

fn theme_is_light(mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Auto => sys::light_taskbar(),
        ThemeMode::Dark => false,
        ThemeMode::Light => true,
    }
}

impl App {
    fn palette(&self) -> &Palette {
        if self.light { &self.cfg.appearance.light } else { &self.cfg.appearance.dark }
    }

    fn fonts(&mut self, dpi: u32) -> &Fonts {
        let a = &self.cfg.appearance;
        let key = (dpi, a.font.clone(), (a.font_size * 10.0) as u32);
        if self.fonts.as_ref().is_none_or(|f| f.key != key) {
            let px = |v: f32| (v * dpi as f32 / 96.0).round() as i32;
            self.fonts = Some(Fonts {
                label: Font::new(&a.font, px(a.font_size), FW_SEMIBOLD as i32),
                tag: Font::new(&a.font, px(a.font_size - 1.0), FW_NORMAL as i32),
                pct: Font::new(&a.font, px(a.font_size), FW_SEMIBOLD as i32),
                key,
            });
        }
        self.fonts.as_ref().unwrap()
    }

    fn metrics(&mut self, dpi: u32) -> Metrics {
        let scale = dpi as f32 / 96.0;
        let s = |v: f32| (v * scale).round() as i32;
        let a = self.cfg.appearance.clone();
        // Every tag a block may show, so the layout does not shift when data arrives.
        let blocks: Vec<(String, Vec<String>)> = self
            .views
            .iter()
            .map(|v| {
                let keys = display::possible_keys(&v.src, v.reading.as_ref());
                let tags =
                    keys.iter().map(|k| display::tag(k, v.src.bars.iter().find(|b| b.keys.contains(k)), v.reading.as_ref())).collect();
                (v.src.label.clone(), tags)
            })
            .collect();
        let dc = self.measure.dc;
        let f = self.fonts(dpi);
        let pct = if a.show_percent { render::text_width(dc, &f.pct, "100%") } else { 0 };
        let labels = blocks.iter().map(|(l, _)| render::text_width(dc, &f.label, l)).collect();
        let tags = blocks
            .iter()
            .map(|(_, tags)| if a.show_tags { tags.iter().map(|t| render::text_width(dc, &f.tag, t)).max().unwrap_or(0) } else { 0 })
            .collect();
        Metrics {
            scale,
            pad: s(4.0),
            labels,
            tags,
            bar: s(a.bar_width),
            pct,
            gap_label: s(7.0),
            gap_tag: s(5.0),
            gap_bar: s(6.0),
            gap_block: s(a.gap),
        }
    }

    fn request_refresh(&mut self) {
        self.refreshing = true;
        self.render();
        self.worker.refresh();
    }

    /// Re-reads the settings file and restarts polling with the new sources.
    fn reload(&mut self) {
        self.config_stamp = config::stamp();
        self.cfg = config::load();
        let light = theme_is_light(self.cfg.theme);
        if light != self.light {
            self.light = light;
            apply_tooltip_theme(self.tip, light);
        }
        self.worker.stop();
        self.remove_tools();
        let previous = std::mem::take(&mut self.views);
        self.views = build_views(&self.cfg, previous);
        self.start_worker();
        self.fonts = None;
        self.placed = None;
        if !self.widget.is_null() {
            self.add_tools();
        }
        self.refreshing = true;
        self.ensure_attached(false);
    }

    fn start_worker(&mut self) {
        self.next_generation += 1;
        self.worker = spawn_worker(&self.views, self.next_generation, self.host);
    }

    /// Creates the widget inside the current taskbar if it is missing, then
    /// keeps its position, size and z-order in step with the taskbar.
    fn ensure_attached(&mut self, force: bool) {
        let tb = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), null()) };
        if tb.is_null() {
            return;
        }
        let alive = !self.widget.is_null() && unsafe { IsWindow(self.widget) } != 0 && unsafe { GetParent(self.widget) } == tb;
        if force || !alive {
            self.remove_tools();
            if !self.widget.is_null() && unsafe { IsWindow(self.widget) } != 0 {
                unsafe { DestroyWindow(self.widget) };
            }
            self.widget = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_NOACTIVATE,
                    wide(WIDGET_CLASS).as_ptr(),
                    wide("Limitbar").as_ptr(),
                    WS_CHILD | WS_CLIPSIBLINGS,
                    0,
                    0,
                    1,
                    1,
                    tb,
                    null_mut(),
                    self.hinst,
                    null(),
                )
            };
            self.taskbar = tb;
            self.placed = None;
            if self.widget.is_null() {
                return;
            }
            self.add_tools();
        }
        self.place();
    }

    fn place(&mut self) {
        let tb = self.taskbar;
        let dpi = match unsafe { GetDpiForWindow(tb) } {
            0 => 96,
            d => d,
        };
        let m = self.metrics(dpi);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(tb, &mut rc) };
        let (tw, th) = (rc.right - rc.left, rc.bottom - rc.top);
        if tw <= 0 || th <= 0 {
            return;
        }
        let w = m.width();
        let h = th;
        let margin = (8.0 * m.scale).round() as i32 + (self.cfg.offset_x as f32 * m.scale).round() as i32;
        let right = match self.cfg.position {
            Position::Auto => sys::taskbar_left_aligned(),
            Position::Left => false,
            Position::Right => true,
        };
        let x = if right {
            // Just before the notification area.
            let tray = unsafe { FindWindowExW(tb, null_mut(), wide("TrayNotifyWnd").as_ptr(), null()) };
            let mut tr = RECT { left: tw, top: 0, right: tw, bottom: 0 };
            if !tray.is_null() {
                unsafe {
                    GetWindowRect(tray, &mut tr);
                    MapWindowPoints(null_mut(), tb, &mut tr as *mut RECT as *mut POINT, 2);
                }
            }
            tr.left - w - margin
        } else {
            margin
        };
        let place = (x, 0, w, h, dpi);
        if self.placed != Some(place) {
            unsafe { SetWindowPos(self.widget, HWND_TOP, x, 0, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
            self.placed = Some(place);
            self.update_tool_rects(&m.blocks(), m.gap_block / 2, h);
            self.render();
        } else if unsafe { GetWindow(tb, GW_CHILD) } != self.widget {
            // Keep the widget above the taskbar's own content bridge.
            unsafe { SetWindowPos(self.widget, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
        }
    }

    fn render(&mut self) {
        let Some((_, _, w, h, dpi)) = self.placed else { return };
        if self.widget.is_null() {
            return;
        }
        let m = self.metrics(dpi);
        if m.width() != w {
            // A source's rows changed (first data for a custom provider): re-layout.
            self.placed = None;
            self.place();
            return;
        }
        if self.painter.as_ref().is_none_or(|p| p.out.w != w || p.out.h != h) {
            self.painter = Painter::new(w, h);
        }
        let now = time::now();
        let palette = *self.palette();
        let light = self.light;
        let refreshing = self.refreshing;
        let blocks = m.blocks();
        let s = m.scale;
        let row_h = (16.0 * s).round() as i32;
        let cfg = &self.cfg;
        let a = &cfg.appearance;
        let bar_h = (a.bar_height * s).round().max(1.0);
        let views = &self.views;
        let fonts = self.fonts.as_ref().unwrap();
        let Some(p) = self.painter.as_mut() else { return };
        p.clear();
        let cy = h as f32 / 2.0;
        for (i, v) in views.iter().enumerate() {
            let (bx, _) = blocks[i];
            let accent = if light { v.src.color.light } else { v.src.color.dark };
            let has_data = v.reading.is_some();
            let old = v.reading.as_ref().is_some_and(|r| now - r.fetched_at > stale_after(&v.src));
            let fade = if old {
                0.5
            } else if refreshing && !v.src.is_system() {
                0.7
            } else {
                1.0
            };
            p.text(&fonts.label, &v.src.label, bx, 0, m.labels[i], h, Align::Left, if has_data { accent } else { accent.fade(0.5) });
            let rows = display::rows(&v.src, v.reading.as_ref());
            let tag_x = bx + m.labels[i] + m.gap_label;
            let bar_x = tag_x + if m.tags[i] > 0 { m.tags[i] + m.gap_tag } else { 0 };
            let pct_x = bar_x + m.bar + m.gap_bar;
            for (row, off) in rows.iter().zip(row_offsets(rows.len())) {
                let rc = (cy + off * s).round();
                let top = (rc - row_h as f32 / 2.0).round() as i32;
                if m.tags[i] > 0 && !row.key.is_empty() {
                    let tag = display::tag(&row.key, row.bar, v.reading.as_ref());
                    p.text(&fonts.tag, &tag, tag_x, top, m.tags[i], row_h, Align::Left, palette.dim);
                }
                let bar_y = (rc - bar_h / 2.0).round();
                p.round_rect(bar_x as f32, bar_y, m.bar as f32, bar_h, bar_h / 2.0, palette.track);
                match row.window {
                    Some(w) => {
                        let used = w.effective(now);
                        let shown = display::shown(&w, now, a.show_remaining);
                        let (fill, text) = severity(used, accent, cfg, &palette);
                        let fw = (m.bar as f32 * shown as f32 / 100.0).max(if shown > 0.0 { bar_h } else { 0.0 });
                        p.round_rect(bar_x as f32, bar_y, fw, bar_h, bar_h / 2.0, fill.fade(fade));
                        if m.pct > 0 {
                            let label = format!("{}%", shown.round() as i64);
                            p.text(&fonts.pct, &label, pct_x, top, m.pct, row_h, Align::Right, text.fade(fade));
                        }
                    }
                    None if m.pct > 0 => p.text(&fonts.pct, "\u{2013}", pct_x, top, m.pct, row_h, Align::Right, palette.dim),
                    None => {}
                }
            }
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        unsafe {
            UpdateLayeredWindow(self.widget, null_mut(), null(), &size, p.out.dc, &src, 0, &blend, ULW_ALPHA);
        }
    }

    fn tip_text(&self, v: &View, now: i64) -> String {
        let mut lines = Vec::new();
        let mut head = v.src.name.clone();
        if let Some(r) = &v.reading {
            for extra in [&r.email, &r.plan].into_iter().flatten() {
                head.push_str(" \u{00B7} ");
                head.push_str(extra);
            }
        }
        lines.push(head);
        let remaining = self.cfg.appearance.show_remaining;
        let system = v.src.is_system();
        for e in display::tooltip_windows(&v.src, v.reading.as_ref()) {
            lines.push(display::window_line(&e, now, remaining, sys::day_clock));
            if let Some(d) = &e.detail {
                lines.push(format!("      {d}"));
            }
        }
        if let Some(r) = &v.reading {
            if let Some(note) = &r.note {
                lines.push(note.clone());
            }
            // Live load needs no timestamp unless it stopped updating.
            if !system || now - r.fetched_at > stale_after(&v.src) {
                lines.push(format!("Updated {}", sys::day_clock(r.fetched_at, now)));
            }
        }
        match &v.status {
            Status::Waiting if system => lines.push("Measuring\u{2026}".into()),
            Status::Waiting => lines.push("Checking usage\u{2026}".into()),
            Status::Live => {}
            Status::Idle(msg) => lines.push(msg.clone()),
            Status::Error(msg) if v.src.id.is_empty() => lines.push(msg.clone()),
            Status::Error(msg) => lines.push(format!("Last check failed: {msg}")),
            Status::Limited(until) => lines.push(format!("The provider is throttling usage checks; next try at {}", sys::clock(*until))),
        }
        for w in self.cfg.warnings.iter().take(3) {
            lines.push(format!("Settings: {w}"));
        }
        if self.cfg.warnings.len() > 3 {
            lines.push(format!("Settings: {} more problems (run limitbar --probe)", self.cfg.warnings.len() - 3));
        }
        lines.push("Click to refresh \u{00B7} right-click for options".into());
        lines.join("\r\n")
    }

    fn tool_info(&self, id: usize) -> TTTOOLINFOW {
        let mut ti: TTTOOLINFOW = unsafe { std::mem::zeroed() };
        ti.cbSize = size_of::<TTTOOLINFOW>() as u32;
        ti.hwnd = self.widget;
        ti.uId = id;
        ti.hinst = self.hinst;
        ti
    }

    fn add_tools(&mut self) {
        let now = time::now();
        self.tips = self.views.iter().map(|v| wide(&self.tip_text(v, now))).collect();
        for i in 0..self.views.len() {
            let mut ti = self.tool_info(i);
            ti.uFlags = TTF_SUBCLASS;
            ti.lpszText = self.tips[i].as_mut_ptr();
            unsafe { SendMessageW(self.tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM) };
        }
    }

    fn remove_tools(&mut self) {
        if self.widget.is_null() {
            return;
        }
        for i in 0..self.tips.len() {
            let ti = self.tool_info(i);
            unsafe { SendMessageW(self.tip, TTM_DELTOOLW, 0, &ti as *const _ as LPARAM) };
        }
        self.tips.clear();
    }

    fn update_tool_rects(&self, blocks: &[(i32, i32)], half_gap: i32, h: i32) {
        for (i, (x, w)) in blocks.iter().enumerate() {
            let mut ti = self.tool_info(i);
            // Each block's hover area runs to the middle of the gap beside it.
            ti.rect = RECT { left: (x - half_gap).max(0), top: 0, right: x + w + half_gap, bottom: h };
            unsafe { SendMessageW(self.tip, TTM_NEWTOOLRECTW, 0, &ti as *const _ as LPARAM) };
        }
    }

    fn update_tips(&mut self) {
        let now = time::now();
        let texts: Vec<Vec<u16>> = self.views.iter().map(|v| wide(&self.tip_text(v, now))).collect();
        for (i, text) in texts.into_iter().enumerate() {
            if i >= self.tips.len() || self.tips[i] == text {
                continue;
            }
            self.tips[i] = text;
            let mut ti = self.tool_info(i);
            ti.lpszText = self.tips[i].as_mut_ptr();
            unsafe { SendMessageW(self.tip, TTM_UPDATETIPTEXTW, 0, &ti as *const _ as LPARAM) };
        }
    }

    fn apply(&mut self, u: Update) {
        if u.generation != self.worker.generation {
            return;
        }
        let Some(v) = self.views.get_mut(u.idx) else { return };
        v.status = match u.outcome {
            Outcome::Fresh(r) => {
                v.reading = Some(r);
                Status::Live
            }
            Outcome::Idle(msg) => Status::Idle(msg),
            Outcome::Failed(msg) => Status::Error(msg),
            Outcome::Limited(until) => Status::Limited(until),
        };
    }

    /// Usage pages for the menu, one per distinct URL, in taskbar order.
    fn pages(&self) -> Vec<(String, String)> {
        let mut pages: Vec<(String, String)> = Vec::new();
        for v in &self.views {
            let page = match (&v.src.url, v.src.is_system()) {
                (Some(url), _) => {
                    let title = v.src.builtin().map_or_else(|| v.src.name.clone(), |b| b.title().to_owned());
                    (format!("Open {title} usage page"), url.clone())
                }
                (None, true) => ("Open Task Manager".to_owned(), "taskmgr.exe".to_owned()),
                (None, false) => continue,
            };
            if !pages.iter().any(|(_, u)| *u == page.1) {
                pages.push(page);
            }
        }
        pages
    }
}

fn edit_settings() {
    let path = config::config_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, config::starter());
    }
    sys::open_text_file(&path.to_string_lossy());
}

fn show_menu(host: HWND) {
    let Some(pages) = with_app(|a| a.pages()) else { return };
    let cmd = unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, CMD_REFRESH, wide("Refresh now").as_ptr());
        if !pages.is_empty() {
            AppendMenuW(m, MF_SEPARATOR, 0, null());
        }
        for (i, (text, _)) in pages.iter().enumerate() {
            AppendMenuW(m, MF_STRING, CMD_PAGE + i, wide(text).as_ptr());
        }
        AppendMenuW(m, MF_SEPARATOR, 0, null());
        AppendMenuW(m, MF_STRING, CMD_SETTINGS, wide("Edit settings\u{2026}").as_ptr());
        let check = if sys::startup_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(m, MF_STRING | check, CMD_STARTUP, wide("Start with Windows").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_EXIT, wide("Exit Limitbar").as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(host);
        // Modal loop: the app state is not borrowed while it runs.
        let cmd = TrackPopupMenuEx(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, host, null());
        PostMessageW(host, WM_NULL, 0, 0);
        DestroyMenu(m);
        cmd as usize
    };
    match cmd {
        CMD_REFRESH => {
            with_app(App::request_refresh);
        }
        CMD_SETTINGS => edit_settings(),
        CMD_STARTUP => sys::set_startup(!sys::startup_enabled()),
        CMD_EXIT => unsafe {
            DestroyWindow(host);
        },
        c if c >= CMD_PAGE => {
            if let Some((_, url)) = pages.get(c - CMD_PAGE) {
                sys::open_url(url);
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn widget_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONUP => {
            with_app(App::request_refresh);
            0
        }
        WM_RBUTTONUP => {
            if let Some(host) = with_app(|a| a.host) {
                show_menu(host);
            }
            0
        }
        WM_NOTIFY if unsafe { (*(lp as *const NMHDR)).code } == TTN_SHOW => {
            unsafe { place_tooltip_above((*(lp as *const NMHDR)).hwndFrom, hwnd) };
            1
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_UPDATE | WM_APP_ROUND_DONE => {
            // Take ownership first so the update is freed even if it cannot be applied.
            let update = unsafe { Box::from_raw(lp as *mut Update) };
            with_app(|a| {
                if msg == WM_APP_UPDATE {
                    a.apply(*update);
                } else if update.generation == a.worker.generation {
                    a.refreshing = false;
                }
                a.render();
                a.update_tips();
            });
            0
        }
        WM_APP_REFRESH => {
            with_app(App::request_refresh);
            0
        }
        WM_TIMER => {
            with_app(|a| match wp {
                TIMER_WATCH => {
                    if config::stamp() != a.config_stamp {
                        a.reload();
                    }
                    a.ensure_attached(false);
                }
                TIMER_TICK => {
                    a.render();
                    a.update_tips();
                }
                _ => {}
            });
            0
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            with_app(|a| {
                let light = theme_is_light(a.cfg.theme);
                if light != a.light {
                    a.light = light;
                    apply_tooltip_theme(a.tip, light);
                }
                a.placed = None;
                a.ensure_attached(false);
            });
            0
        }
        WM_POWERBROADCAST => {
            if wp == PBT_APMRESUMEAUTOMATIC as usize {
                with_app(App::request_refresh);
            }
            1
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            with_app(|a| {
                a.worker.stop();
                if !a.widget.is_null() {
                    unsafe { DestroyWindow(a.widget) };
                    a.widget = null_mut();
                }
            });
            unsafe { PostQuitMessage(0) };
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted: the old taskbar and our child went with it.
            with_app(|a| a.ensure_attached(true));
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Shows the tooltip just above the taskbar, centred on the cursor, instead of
/// the default spot below the cursor where it would cover the widget.
unsafe fn place_tooltip_above(tip: HWND, widget: HWND) {
    unsafe {
        let mut tr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut wr = tr;
        GetWindowRect(tip, &mut tr);
        GetWindowRect(widget, &mut wr);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(widget, MONITOR_DEFAULTTONEAREST), &mut mi);
        let (tw, th) = (tr.right - tr.left, tr.bottom - tr.top);
        let gap = (6 * GetDpiForWindow(widget).max(96) / 96) as i32;
        let x = (pt.x - tw / 2).clamp(mi.rcMonitor.left + gap, (mi.rcMonitor.right - tw - gap).max(mi.rcMonitor.left));
        let y = wr.top - th - gap;
        SetWindowPos(tip, null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn apply_tooltip_theme(tip: HWND, light: bool) {
    let theme = if light { wide("Explorer") } else { wide("DarkMode_Explorer") };
    unsafe { SetWindowTheme(tip, theme.as_ptr(), null()) };
    set_menu_theme(light);
}

/// Matches popup menus to the taskbar theme through uxtheme's
/// SetPreferredAppMode (ordinal 135) and FlushMenuThemes (ordinal 136), the
/// same calls Explorer-style apps use. Missing exports are simply skipped.
fn set_menu_theme(light: bool) {
    unsafe {
        let ux = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if ux.is_null() {
            return;
        }
        if let Some(set_mode) = GetProcAddress(ux, 135 as _) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            set_mode(if light { 3 } else { 2 }); // ForceLight / ForceDark
        }
        if let Some(flush) = GetProcAddress(ux, 136 as _) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

fn register_class(hinst: HINSTANCE, name: &[u16], proc_: WNDPROC) {
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: proc_,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: name.as_ptr(),
        hIconSm: null_mut(),
    };
    unsafe { RegisterClassExW(&wc) };
}

/// Lets console commands print when started from a terminal: a GUI-subsystem
/// exe has no stdout unless it was redirected.
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// `println!` that ignores a closed pipe (`limitbar --probe | Select -First 3`).
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// `limitbar --probe`: checks every configured source once, prints one line
/// per source (label, windows, reset times, plan, provider note) and exits.
/// Never prints tokens or account names and does not touch the cache or a
/// running instance. Exit code 1 if any source failed.
fn probe() -> i32 {
    let cfg = config::load();
    out!("Settings: {}{}", config::config_path().display(), if config::stamp().is_some() { "" } else { " (not present: defaults)" });
    for w in &cfg.warnings {
        out!("  warning: {w}");
    }
    let Some(http) = net::Http::new(providers::user_agent()) else {
        out!("WinHTTP is unavailable");
        return 1;
    };
    if cfg.sources.is_empty() {
        out!("No sources: {NO_SOURCES}");
        return 1;
    }
    let width = cfg.sources.iter().map(|s| s.name.chars().count()).max().unwrap_or(0).max(8) + 2;
    let mut failed = !cfg.warnings.is_empty();
    for src in &cfg.sources {
        let outcome = providers::fetch(&http, src, &mut Profile::default());
        let now = time::now();
        let detail = match &outcome {
            Outcome::Fresh(r) => {
                let mut parts: Vec<String> = display::tooltip_windows(src, Some(r))
                    .into_iter()
                    .map(|e| {
                        let bar = src.bars.iter().find(|b| b.keys.contains(&e.key));
                        let tag = display::tag(&e.key, bar, Some(r));
                        match e.window {
                            Some(w) if src.is_system() => {
                                let amounts = match (w.value, w.limit, &e.unit) {
                                    (Some(v), Some(l), Some(u)) => format!(" ({v} of {l} {u})"),
                                    _ => String::new(),
                                };
                                format!("{tag} {:.1}%{amounts}", w.used)
                            }
                            Some(w) if w.resets_at > 0 => format!(
                                "{tag} {:.1}% (resets {}, in {})",
                                w.used,
                                sys::day_clock(w.resets_at, now),
                                time::countdown(w.resets_at - now)
                            ),
                            Some(w) => format!("{tag} {:.1}%", w.used),
                            None => format!("{tag} not reported"),
                        }
                    })
                    .collect();
                parts.extend(r.plan.as_ref().map(|p| format!("plan {p}")));
                parts.extend(r.note.as_ref().map(|n| format!("note \"{n}\"")));
                format!("ok     {}", parts.join(" | "))
            }
            Outcome::Idle(msg) => format!("idle   {msg}"),
            Outcome::Failed(msg) => {
                failed = true;
                format!("FAILED {msg}")
            }
            Outcome::Limited(until) => format!("429    throttled until {}", sys::clock(*until)),
        };
        out!("{:<5}{:<width$}{detail}", src.label, src.name);
        // The busiest processes and other details a system source shows in its tooltip.
        if let Outcome::Fresh(r) = &outcome
            && src.is_system()
        {
            for e in display::tooltip_windows(src, Some(r)) {
                let tag = display::tag(&e.key, src.bars.iter().find(|b| b.keys.contains(&e.key)), Some(r));
                for extra in [&e.info, &e.detail].into_iter().flatten() {
                    out!("{:<5}{:<width$}       {tag}: {extra}", "", "");
                }
            }
        }
    }
    i32::from(failed)
}

const HELP: &str = "Limitbar shows AI plan usage (or any quota) in the Windows taskbar.

  limitbar            start the widget (or refresh the running one)
  limitbar --probe    check every source once and print the results
  limitbar --exit     stop the running widget
  limitbar --version  print the version

Settings: %APPDATA%\\Limitbar\\config.json (right-click the widget > Edit settings).
Docs: https://github.com/vsolano9/limitbar";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--probe") || has("--help") || has("-h") || has("/?") || has("--version") || has("-V") {
        attach_console();
        if has("--probe") {
            std::process::exit(probe());
        }
        if has("--version") || has("-V") {
            println!("limitbar {}", env!("CARGO_PKG_VERSION"));
        } else {
            println!("{HELP}");
        }
        return;
    }
    let exit_request = has("--exit");
    unsafe {
        let _mutex = CreateMutexW(null(), 0, wide("Local\\Limitbar.SingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS || exit_request {
            let host = FindWindowW(wide(HOST_CLASS).as_ptr(), null());
            if !host.is_null() {
                PostMessageW(host, if exit_request { WM_CLOSE } else { WM_APP_REFRESH }, 0, 0);
            }
            return;
        }

        let hinst = GetModuleHandleW(null()) as HINSTANCE;
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        InitCommonControlsEx(&icc);
        let host_class = wide(HOST_CLASS);
        let widget_class = wide(WIDGET_CLASS);
        register_class(hinst, &host_class, Some(host_proc));
        register_class(hinst, &widget_class, Some(widget_proc));
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);

        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            host_class.as_ptr(),
            wide("Limitbar").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if host.is_null() {
            return;
        }
        let tip = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            TOOLTIPS_CLASSW,
            null(),
            WS_POPUP | TTS_NOPREFIX | TTS_ALWAYSTIP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            host,
            null_mut(),
            hinst,
            null(),
        );
        // Wide enough for "Cursor models (Auto + Composer): ... resets Thu Oct 15 ..." on one line.
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, (640 * GetDpiForSystem() / 96) as isize);
        SendMessageW(tip, TTM_SETDELAYTIME, TTDT_AUTOPOP as usize, 30_000);

        let config_stamp = config::stamp();
        let cfg = config::load();
        let light = theme_is_light(cfg.theme);
        apply_tooltip_theme(tip, light);
        let views = build_views(&cfg, Vec::new());
        let Some(measure) = Surface::new(1, 1) else { return };
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                hinst,
                host,
                widget: null_mut(),
                tip,
                taskbar: null_mut(),
                cfg,
                config_stamp,
                worker: spawn_worker(&views, 1, host),
                views,
                next_generation: 1,
                refreshing: true,
                light,
                placed: None,
                fonts: None,
                measure,
                painter: None,
                tips: Vec::new(),
            })
        });
        with_app(|a| a.ensure_attached(false));
        SetTimer(host, TIMER_WATCH, 2_000, None);
        SetTimer(host, TIMER_TICK, 60_000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| a.borrow_mut().take());
    }
}
