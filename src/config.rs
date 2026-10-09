//! Settings: which sources to show, how they look, and where state lives.
//!
//! Without a settings file Limitbar shows every built-in provider it finds
//! signed in. `%APPDATA%\Limitbar\config.json` (JSON with comments) can list
//! sources explicitly, add `http`, `command` and `file` providers for anything
//! else, and change the appearance. See docs/configuration.md.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::jsonc;
use crate::render::{Rgba, rgba};
use crate::usage::{CURSOR_API, CURSOR_AUTO, DAILY, FIVE_HOUR, MONTHLY, WEEKLY};

pub const MIN_POLL: u64 = 60;
/// Custom providers may poll faster (a local script or file costs nothing).
pub const MIN_POLL_CUSTOM: u64 = 15;
const DEFAULT_POLL: u64 = 120;
/// Rows that fit in the taskbar's height.
pub const MAX_BARS: usize = 3;
pub const SCHEMA_URL: &str = "https://raw.githubusercontent.com/vsolano9/limitbar/main/limitbar.schema.json";

// ---------------------------------------------------------------- built-ins

/// Providers with code of their own in `providers/`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Builtin {
    Claude,
    Codex,
    Grok,
    Cursor,
}

impl Builtin {
    pub const ALL: [Builtin; 4] = [Builtin::Claude, Builtin::Codex, Builtin::Grok, Builtin::Cursor];

    fn parse(s: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|k| k.key() == s)
    }

    /// The `provider` value in `config.json`.
    pub fn key(self) -> &'static str {
        match self {
            Builtin::Claude => "claude",
            Builtin::Codex => "codex",
            Builtin::Grok => "grok",
            Builtin::Cursor => "cursor",
        }
    }

    /// The provider's name, e.g. for "Open Claude usage page".
    pub fn title(self) -> &'static str {
        self.default_label().1
    }

    /// Default taskbar label and tooltip name.
    fn default_label(self) -> (&'static str, &'static str) {
        match self {
            Builtin::Claude => ("CL", "Claude"),
            Builtin::Codex => ("CX", "Codex"),
            Builtin::Grok => ("GK", "Grok"),
            Builtin::Cursor => ("CU", "Cursor"),
        }
    }

    /// The file whose presence means the CLI is signed in.
    pub fn credential_file(self) -> &'static str {
        match self {
            Builtin::Claude => ".credentials.json",
            _ => "auth.json",
        }
    }

    /// Rows drawn on the taskbar, top to bottom. Each row shows the first of
    /// its keys the reading has (Grok reports one period whose length the
    /// server decides).
    fn default_bars(self) -> &'static [&'static [&'static str]] {
        match self {
            Builtin::Claude => &[&[FIVE_HOUR], &[WEEKLY]],
            Builtin::Codex => &[&[WEEKLY]],
            Builtin::Grok => &[&[WEEKLY, MONTHLY, DAILY]],
            Builtin::Cursor => &[&[CURSOR_AUTO], &[CURSOR_API]],
        }
    }

    /// The provider's own usage page, offered in the right-click menu.
    fn usage_url(self) -> &'static str {
        match self {
            Builtin::Claude => "https://claude.ai/settings/usage",
            Builtin::Codex => "https://chatgpt.com/codex/settings/usage",
            Builtin::Grok => "https://grok.com/?_s=usage",
            Builtin::Cursor => "https://cursor.com/dashboard?tab=usage",
        }
    }

    fn color(self) -> ColorPair {
        match self {
            Builtin::Claude => ColorPair { dark: rgba(232, 142, 112, 1.0), light: rgba(190, 88, 52, 1.0) },
            Builtin::Codex => ColorPair { dark: rgba(150, 168, 255, 1.0), light: rgba(64, 86, 214, 1.0) },
            Builtin::Grok => ColorPair { dark: rgba(204, 210, 222, 1.0), light: rgba(82, 90, 106, 1.0) },
            Builtin::Cursor => ColorPair { dark: rgba(96, 208, 168, 1.0), light: rgba(0, 132, 102, 1.0) },
        }
    }

    /// Where each CLI keeps its sign-in by default.
    fn default_dir(self, home: &Path, appdata: &Path) -> PathBuf {
        match self {
            Builtin::Claude => env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            Builtin::Codex => env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            Builtin::Grok => home.join(".grok"),
            Builtin::Cursor => appdata.join("Cursor"),
        }
    }
}

/// Colours for custom sources without a `color`, in order.
const PALETTE: [ColorPair; 6] = [
    ColorPair { dark: rgba(92, 205, 214, 1.0), light: rgba(0, 120, 132, 1.0) },
    ColorPair { dark: rgba(196, 160, 255, 1.0), light: rgba(112, 64, 200, 1.0) },
    ColorPair { dark: rgba(244, 146, 204, 1.0), light: rgba(176, 48, 120, 1.0) },
    ColorPair { dark: rgba(120, 196, 255, 1.0), light: rgba(16, 108, 188, 1.0) },
    ColorPair { dark: rgba(176, 222, 112, 1.0), light: rgba(72, 128, 16, 1.0) },
    ColorPair { dark: rgba(232, 214, 150, 1.0), light: rgba(128, 104, 32, 1.0) },
];

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPair {
    pub dark: Rgba,
    pub light: Rgba,
}

/// Where a value (usually a secret) comes from. Secrets never live in Limitbar's
/// own files: they are read when a request is made and dropped afterwards.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueSource {
    Literal(String),
    Env(String),
    /// A file's trimmed text, or one value inside it when `json` is a path.
    File {
        path: PathBuf,
        json: Option<String>,
    },
    /// The password of a generic Windows Credential Manager entry.
    Credential(String),
    /// A command's trimmed standard output.
    Command(CommandLine),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandLine {
    /// Run through `cmd.exe /d /s /c`.
    Shell(String),
    /// Program and arguments, run directly.
    Argv(Vec<String>),
}

/// When the stored sign-in expires; past it the source is idle (shows its last
/// values) instead of failing.
#[derive(Clone, Debug, PartialEq)]
pub enum Expiry {
    /// A timestamp (ISO 8601, unix seconds or milliseconds).
    At(ValueSource),
    /// The `exp` claim of the JWT in this variable.
    Jwt(String),
}

/// A number in a response: a path into it, or a constant.
#[derive(Clone, Debug, PartialEq)]
pub enum Amount {
    Path(String),
    Const(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub key: String,
    pub tag: Option<String>,
    pub name: Option<String>,
    pub unit: Option<String>,
    pub percent: Option<Amount>,
    pub used: Option<Amount>,
    pub remaining: Option<Amount>,
    pub limit: Option<Amount>,
    pub scale: f64,
    /// Used when the amounts are missing (protobuf JSON omits zeros).
    pub default: Option<f64>,
    pub resets_at: Option<String>,
    pub resets_in: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HttpSpec {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub vars: Vec<(String, ValueSource)>,
    pub expires: Option<Expiry>,
    pub windows: Vec<WindowSpec>,
    pub plan: Option<String>,
    pub account: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Provider {
    Builtin { kind: Builtin, dir: PathBuf },
    Http(Box<HttpSpec>),
    Command { line: CommandLine, timeout: u64 },
    File { path: PathBuf },
}

/// One taskbar row: the first of `keys` the reading has, with optional
/// display overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub keys: Vec<String>,
    pub tag: Option<String>,
    pub name: Option<String>,
    pub unit: Option<String>,
}

impl Bar {
    fn key(k: &str) -> Bar {
        Bar { keys: vec![k.to_owned()], tag: None, name: None, unit: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// Stable key for the cache file.
    pub id: String,
    /// Short tag drawn on the taskbar, e.g. `C1`.
    pub label: String,
    /// Full name for the tooltip, e.g. `Claude 1`.
    pub name: String,
    pub provider: Provider,
    /// Rows to draw. Empty: the windows the provider reports, up to three.
    pub bars: Vec<Bar>,
    pub color: ColorPair,
    pub url: Option<String>,
    pub poll_seconds: u64,
}

impl Source {
    pub fn builtin(&self) -> Option<Builtin> {
        match self.provider {
            Provider::Builtin { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    /// Left end of a centred taskbar; next to the clock when Start is on the left.
    Auto,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeMode {
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub text: Rgba,
    pub dim: Rgba,
    pub track: Rgba,
    pub warn: Rgba,
    pub critical: Rgba,
}

pub const DARK: Palette = Palette {
    text: rgba(255, 255, 255, 0.95),
    dim: rgba(255, 255, 255, 0.58),
    track: rgba(255, 255, 255, 0.16),
    warn: rgba(247, 185, 85, 1.0),
    critical: rgba(255, 107, 97, 1.0),
};

pub const LIGHT: Palette = Palette {
    text: rgba(0, 0, 0, 0.90),
    dim: rgba(0, 0, 0, 0.56),
    track: rgba(0, 0, 0, 0.12),
    warn: rgba(176, 112, 0, 1.0),
    critical: rgba(200, 40, 30, 1.0),
};

#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub font: String,
    /// Label and percentage size in points at 100% scaling; row tags are one smaller.
    pub font_size: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    /// Space between sources.
    pub gap: f32,
    pub show_tags: bool,
    pub show_percent: bool,
    /// Show what is left (100% = untouched) instead of what is used.
    pub show_remaining: bool,
    pub warn_at: f64,
    pub critical_at: f64,
    pub dark: Palette,
    pub light: Palette,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            font: "Segoe UI Variable Text".into(),
            font_size: 12.0,
            bar_width: 44.0,
            bar_height: 4.0,
            gap: 18.0,
            show_tags: true,
            show_percent: true,
            show_remaining: false,
            warn_at: 75.0,
            critical_at: 90.0,
            dark: DARK,
            light: LIGHT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Extra horizontal offset in logical pixels (e.g. to clear the Widgets button).
    pub offset_x: i32,
    pub position: Position,
    pub theme: ThemeMode,
    pub appearance: Appearance,
    pub sources: Vec<Source>,
    /// Problems found in the settings file, shown in the tooltip and by `--probe`.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- paths

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn home() -> PathBuf {
    env_path("USERPROFILE").unwrap_or_else(|| PathBuf::from("C:\\"))
}

/// Roaming application data (`%APPDATA%`).
pub fn appdata() -> PathBuf {
    env_path("APPDATA").unwrap_or_else(|| home().join("AppData").join("Roaming"))
}

pub fn config_path() -> PathBuf {
    appdata().join("Limitbar").join("config.json")
}

pub fn state_path() -> PathBuf {
    env_path("LOCALAPPDATA").unwrap_or_else(home).join("Limitbar").join("state.json")
}

/// Expands a leading `~` and `%VARIABLE%` references.
pub fn expand_path(s: &str, home: &Path) -> PathBuf {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    match out.strip_prefix('~') {
        Some(tail) if tail.is_empty() || tail.starts_with(['/', '\\']) => home.join(tail.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(out),
    }
}

// ---------------------------------------------------------------- discovery

fn builtin_source(kind: Builtin, id: String, label: String, name: String, dir: PathBuf) -> Source {
    Source {
        id,
        label,
        name,
        provider: Provider::Builtin { kind, dir },
        bars: kind
            .default_bars()
            .iter()
            .map(|keys| Bar { keys: keys.iter().map(|k| (*k).to_owned()).collect(), tag: None, name: None, unit: None })
            .collect(),
        color: kind.color(),
        url: Some(kind.usage_url().to_owned()),
        poll_seconds: DEFAULT_POLL,
    }
}

/// Every Claude Code config directory with a login (`~/.claude` or
/// `CLAUDE_CONFIG_DIR`, then `~/.claude-<name>` directories), then Codex, Grok
/// and Cursor, each if it is signed in. `.claude-account<N>` directories are
/// shown as `C<N>`; several logins are numbered in order.
pub fn discover(home: &Path, appdata: &Path) -> Vec<Source> {
    let signed_in = |dir: &Path, kind: Builtin| dir.join(kind.credential_file()).is_file();
    let suffix_of = |dir: &Path| dir.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(".claude-")).unwrap_or("").to_owned();
    let main = Builtin::Claude.default_dir(home, appdata);
    let mut claude: Vec<(String, PathBuf)> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|dir| !suffix_of(dir).is_empty() && *dir != main)
        .chain(std::iter::once(main.clone()))
        .filter(|dir| signed_in(dir, Builtin::Claude))
        .map(|dir| (suffix_of(&dir), dir))
        .collect();
    // The default directory first, then natural order: account2 before account10.
    claude.sort_by_key(|(s, _)| {
        let digits: String = s.chars().filter(char::is_ascii_digit).collect();
        (!s.is_empty(), s.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned(), digits.parse::<u64>().unwrap_or(0), s.clone())
    });
    let several = claude.len() > 1;
    let mut out: Vec<Source> = claude
        .into_iter()
        .enumerate()
        .map(|(i, (suffix, dir))| {
            let n = i + 1;
            let account = suffix.strip_prefix("account").and_then(|d| d.parse::<u32>().ok());
            let (id, label, name) = match (account, suffix.is_empty(), several) {
                (Some(a), _, _) => (format!("claude{a}"), format!("C{a}"), format!("Claude {a}")),
                (None, true, false) => ("claude".into(), "CL".into(), "Claude".into()),
                (None, true, true) => ("claude".into(), format!("C{n}"), "Claude".into()),
                (None, false, _) => (format!("claude-{suffix}"), format!("C{n}"), format!("Claude ({suffix})")),
            };
            builtin_source(Builtin::Claude, id, label, name, dir)
        })
        .collect();
    for kind in [Builtin::Codex, Builtin::Grok, Builtin::Cursor] {
        let dir = kind.default_dir(home, appdata);
        if signed_in(&dir, kind) {
            let (label, name) = kind.default_label();
            out.push(builtin_source(kind, kind.key().into(), label.into(), name.into(), dir));
        }
    }
    out
}

// ---------------------------------------------------------------- parsing

/// `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let h = s.trim().strip_prefix('#')?;
    if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let long: String;
    let h = match h.len() {
        3 | 4 => {
            long = h.chars().flat_map(|c| [c, c]).collect();
            long.as_str()
        }
        6 | 8 => h,
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let a = if h.len() == 8 { f32::from(byte(6)?) / 255.0 } else { 1.0 };
    Some(rgba(byte(0)?, byte(2)?, byte(4)?, a))
}

struct Parser<'a> {
    home: &'a Path,
    appdata: &'a Path,
    warnings: Vec<String>,
}

fn str_of<'v>(v: &'v Value, k: &str) -> Option<&'v str> {
    v.get(k).and_then(Value::as_str)
}

fn string_of(v: &Value, k: &str) -> Option<String> {
    str_of(v, k).map(str::to_owned)
}

impl Parser<'_> {
    fn warn(&mut self, w: String) {
        self.warnings.push(w);
    }

    fn color_pair(&mut self, v: Option<&Value>, what: &str) -> Option<ColorPair> {
        let v = v?;
        let one = |p: &mut Self, v: Option<&Value>| -> Option<Rgba> {
            let s = v?.as_str();
            let c = s.and_then(parse_color);
            if c.is_none() {
                p.warn(format!("{what}: colour {v:?} is not a #RRGGBB colour", v = s.unwrap_or("?")));
            }
            c
        };
        if v.is_string() {
            let c = one(self, Some(v))?;
            return Some(ColorPair { dark: c, light: c });
        }
        let dark = one(self, v.get("dark"))?;
        let light = v.get("light").map_or(Some(dark), |l| one(self, Some(l)))?;
        Some(ColorPair { dark, light })
    }

    fn command_line(&mut self, v: Option<&Value>) -> Option<CommandLine> {
        match v? {
            Value::String(s) if !s.trim().is_empty() => Some(CommandLine::Shell(s.clone())),
            Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_string) => {
                Some(CommandLine::Argv(a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()))
            }
            _ => None,
        }
    }

    fn value_source(&mut self, v: &Value, what: &str) -> Option<ValueSource> {
        if let Some(s) = v.as_str() {
            return Some(ValueSource::Literal(s.to_owned()));
        }
        if let Some(e) = str_of(v, "env") {
            return Some(ValueSource::Env(e.to_owned()));
        }
        if let Some(f) = str_of(v, "file") {
            return Some(ValueSource::File { path: expand_path(f, self.home), json: string_of(v, "json") });
        }
        if let Some(c) = str_of(v, "credential") {
            return Some(ValueSource::Credential(c.to_owned()));
        }
        if let Some(c) = self.command_line(v.get("command")) {
            return Some(ValueSource::Command(c));
        }
        self.warn(format!("{what}: expected a string or one of env, file, credential, command"));
        None
    }

    fn amount(&mut self, v: Option<&Value>) -> Option<Amount> {
        match v? {
            Value::String(s) => Some(Amount::Path(s.clone())),
            n => n.as_f64().map(Amount::Const),
        }
    }

    fn window_spec(&mut self, v: &Value, i: usize, what: &str) -> Option<WindowSpec> {
        let key = string_of(v, "key").unwrap_or_else(|| format!("w{}", i + 1));
        let spec = WindowSpec {
            tag: string_of(v, "tag"),
            name: string_of(v, "name"),
            unit: string_of(v, "unit"),
            percent: self.amount(v.get("percent")),
            used: self.amount(v.get("used")),
            remaining: self.amount(v.get("remaining")),
            limit: self.amount(v.get("limit")),
            scale: v.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
            default: v.get("default").and_then(Value::as_f64),
            resets_at: string_of(v, "resets_at"),
            resets_in: string_of(v, "resets_in"),
            key,
        };
        if spec.percent.is_none() && spec.used.is_none() && spec.remaining.is_none() {
            self.warn(format!("{what}: window \"{}\" needs percent, used or remaining", spec.key));
            return None;
        }
        Some(spec)
    }

    fn http_spec(&mut self, v: &Value, what: &str) -> Option<HttpSpec> {
        let Some(url) = string_of(v, "url") else {
            self.warn(format!("{what}: an http provider needs a \"url\""));
            return None;
        };
        let headers = v
            .get("headers")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
            .unwrap_or_default();
        let body = match v.get("body") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Null) | None => None,
            Some(other) => Some(other.to_string()),
        };
        let mut vars = Vec::new();
        if let Some(m) = v.get("vars").and_then(Value::as_object) {
            for (name, src) in m {
                if let Some(s) = self.value_source(src, &format!("{what}: var \"{name}\"")) {
                    vars.push((name.clone(), s));
                }
            }
        }
        let expires = match v.get("expires") {
            Some(e) => match str_of(e, "jwt") {
                Some(var) => Some(Expiry::Jwt(var.to_owned())),
                None => self.value_source(e, &format!("{what}: expires")).map(Expiry::At),
            },
            None => None,
        };
        let windows: Vec<WindowSpec> = v
            .get("windows")
            .and_then(Value::as_array)
            .map(|list| list.iter().enumerate().filter_map(|(i, w)| self.window_spec(w, i, what)).collect())
            .unwrap_or_default();
        if windows.is_empty() {
            self.warn(format!("{what}: an http provider needs at least one entry in \"windows\""));
            return None;
        }
        Some(HttpSpec {
            method: string_of(v, "method").map_or_else(|| if body.is_some() { "POST".into() } else { "GET".into() }, |m| m.to_uppercase()),
            url,
            headers,
            body,
            vars,
            expires,
            windows,
            plan: string_of(v, "plan"),
            account: string_of(v, "account"),
            note: string_of(v, "note"),
        })
    }

    fn bars(&mut self, v: Option<&Value>, what: &str) -> Option<Vec<Bar>> {
        let list = v?.as_array()?;
        let mut bars: Vec<Bar> = list
            .iter()
            .filter_map(|b| match b {
                Value::String(k) => Some(Bar::key(k)),
                Value::Object(_) => {
                    let keys = match b.get("key") {
                        Some(Value::String(k)) => vec![k.clone()],
                        Some(Value::Array(a)) => a.iter().filter_map(|k| k.as_str().map(str::to_owned)).collect(),
                        _ => return None,
                    };
                    (!keys.is_empty()).then(|| Bar {
                        keys,
                        tag: string_of(b, "tag"),
                        name: string_of(b, "name"),
                        unit: string_of(b, "unit"),
                    })
                }
                _ => None,
            })
            .collect();
        if bars.len() > MAX_BARS {
            self.warn(format!("{what}: only {MAX_BARS} bars fit in the taskbar; the rest are listed in the tooltip"));
            bars.truncate(MAX_BARS);
        }
        Some(bars)
    }

    /// One entry of `sources`. `None` with a warning when it cannot be used.
    fn source(&mut self, v: &Value, n: usize, palette_next: &mut usize, default_poll: u64) -> Option<Source> {
        let what = format!("source {n}");
        let kind_text = str_of(v, "provider").or_else(|| str_of(v, "kind"));
        let Some(kind_text) = kind_text else {
            self.warn(format!("{what}: missing \"provider\""));
            return None;
        };
        let given_label = str_of(v, "label");
        let given_name = str_of(v, "name");
        let (provider, def_label, def_name) = if let Some(kind) = Builtin::parse(kind_text) {
            let dir = str_of(v, "dir").map_or_else(|| kind.default_dir(self.home, self.appdata), |d| expand_path(d, self.home));
            let (l, nm) = kind.default_label();
            (Provider::Builtin { kind, dir }, l.to_owned(), nm.to_owned())
        } else {
            let provider = match kind_text {
                "http" => Provider::Http(Box::new(self.http_spec(v, &what)?)),
                "command" => {
                    let Some(line) = self.command_line(v.get("command")) else {
                        self.warn(format!("{what}: a command provider needs a \"command\" (string or list)"));
                        return None;
                    };
                    Provider::Command { line, timeout: v.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(30).clamp(1, 600) }
                }
                "file" => {
                    let Some(p) = str_of(v, "path") else {
                        self.warn(format!("{what}: a file provider needs a \"path\""));
                        return None;
                    };
                    Provider::File { path: expand_path(p, self.home) }
                }
                other => {
                    self.warn(format!(
                        "{what}: unknown provider \"{other}\" (expected claude, codex, grok, cursor, http, command or file)"
                    ));
                    return None;
                }
            };
            let fallback = given_name.or(given_label).unwrap_or("Custom");
            let label: String = fallback.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
            (provider, if label.is_empty() { "??".into() } else { label }, fallback.to_owned())
        };
        let label = given_label.map_or(def_label, str::to_owned);
        // As in 1.x, a missing name falls back to the given label (this keeps cache ids stable).
        let name = given_name.or(given_label).map_or(def_name, str::to_owned);
        let id = str_of(v, "id").map_or_else(|| name.to_lowercase().replace(' ', ""), str::to_owned);
        let builtin = match &provider {
            Provider::Builtin { kind, .. } => Some(*kind),
            _ => None,
        };
        let mut bars = self.bars(v.get("bars"), &what).unwrap_or_else(|| match (&provider, builtin) {
            (_, Some(kind)) => builtin_source(kind, String::new(), String::new(), String::new(), PathBuf::new()).bars,
            (Provider::Http(h), None) => h
                .windows
                .iter()
                .take(MAX_BARS)
                .map(|w| Bar { keys: vec![w.key.clone()], tag: w.tag.clone(), name: w.name.clone(), unit: w.unit.clone() })
                .collect(),
            _ => Vec::new(),
        });
        // Display overrides from an http window apply to bars that name it.
        if let Provider::Http(h) = &provider {
            for bar in &mut bars {
                if let Some(w) = h.windows.iter().find(|w| bar.keys.contains(&w.key)) {
                    bar.tag = bar.tag.take().or_else(|| w.tag.clone());
                    bar.name = bar.name.take().or_else(|| w.name.clone());
                    bar.unit = bar.unit.take().or_else(|| w.unit.clone());
                }
            }
        }
        let color = self.color_pair(v.get("color"), &what).unwrap_or_else(|| match builtin {
            Some(kind) => kind.color(),
            None => {
                let c = PALETTE[*palette_next % PALETTE.len()];
                *palette_next += 1;
                c
            }
        });
        let min = if builtin.is_some() { MIN_POLL } else { MIN_POLL_CUSTOM };
        let poll_seconds = v.get("poll_seconds").and_then(Value::as_u64).unwrap_or(default_poll).max(min);
        let url = string_of(v, "page").or_else(|| builtin.map(|k| k.usage_url().to_owned()));
        Some(Source { id, label, name, provider, bars, color, url, poll_seconds })
    }
}

fn dedupe_ids(sources: &mut [Source]) {
    for i in 1..sources.len() {
        let base = sources[i].id.clone();
        let mut n = 2;
        while sources[..i].iter().any(|s| s.id == sources[i].id) {
            sources[i].id = format!("{base}-{n}");
            n += 1;
        }
    }
}

fn appearance(p: &mut Parser, v: Option<&Value>) -> Appearance {
    let mut a = Appearance::default();
    let Some(v) = v else { return a };
    let num = |k: &str, lo: f64, hi: f64| v.get(k).and_then(Value::as_f64).map(|n| n.clamp(lo, hi));
    if let Some(f) = str_of(v, "font").filter(|f| !f.trim().is_empty()) {
        a.font = f.to_owned();
    }
    a.font_size = num("font_size", 7.0, 20.0).map_or(a.font_size, |n| n as f32);
    a.bar_width = num("bar_width", 8.0, 200.0).map_or(a.bar_width, |n| n as f32);
    a.bar_height = num("bar_height", 1.0, 12.0).map_or(a.bar_height, |n| n as f32);
    a.gap = num("gap", 0.0, 80.0).map_or(a.gap, |n| n as f32);
    let flag = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    a.show_tags = flag("show_tags", a.show_tags);
    a.show_percent = flag("show_percent", a.show_percent);
    a.show_remaining = match str_of(v, "show") {
        Some("remaining") => true,
        Some("used") | None => false,
        Some(other) => {
            p.warn(format!("appearance.show: \"{other}\" is not \"used\" or \"remaining\""));
            false
        }
    };
    a.warn_at = num("warn_at", 0.0, 100.0).unwrap_or(a.warn_at);
    a.critical_at = num("critical_at", 0.0, 100.0).unwrap_or(a.critical_at);
    for (mode, palette) in [("dark", &mut a.dark), ("light", &mut a.light)] {
        let Some(c) = v.get("colors").and_then(|c| c.get(mode)) else { continue };
        for (k, slot) in [
            ("text", &mut palette.text),
            ("dim", &mut palette.dim),
            ("track", &mut palette.track),
            ("warn", &mut palette.warn),
            ("critical", &mut palette.critical),
        ] {
            if let Some(s) = str_of(c, k) {
                match parse_color(s) {
                    Some(col) => *slot = col,
                    None => p.warn(format!("appearance.colors.{mode}.{k}: \"{s}\" is not a #RRGGBB colour")),
                }
            }
        }
    }
    a
}

pub fn from_json(v: Option<&Value>, home: &Path, appdata: &Path) -> Config {
    let mut p = Parser { home, appdata, warnings: Vec::new() };
    let get = |k: &str| v.and_then(|v| v.get(k));
    let poll = get("poll_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_POLL).max(MIN_POLL);
    let offset_x = get("offset_x").and_then(Value::as_i64).unwrap_or(0).clamp(-4000, 4000) as i32;
    let position = match get("position").and_then(Value::as_str) {
        Some("left") => Position::Left,
        Some("right") => Position::Right,
        Some("auto") | None => Position::Auto,
        Some(other) => {
            p.warn(format!("position: \"{other}\" is not auto, left or right"));
            Position::Auto
        }
    };
    let theme = match get("theme").and_then(Value::as_str) {
        Some("dark") => ThemeMode::Dark,
        Some("light") => ThemeMode::Light,
        Some("auto") | None => ThemeMode::Auto,
        Some(other) => {
            p.warn(format!("theme: \"{other}\" is not auto, dark or light"));
            ThemeMode::Auto
        }
    };
    let appearance = appearance(&mut p, get("appearance"));
    let sources = match get("sources").and_then(Value::as_array) {
        None => {
            let mut d = discover(home, appdata);
            for s in &mut d {
                s.poll_seconds = poll;
            }
            d
        }
        Some(list) => {
            let mut palette_next = 0;
            // Explicit entries first, so "auto" can skip ids already listed (or hidden).
            let mut slots: Vec<Option<Source>> = Vec::new();
            let mut taken: Vec<String> = Vec::new();
            let mut auto_at: Option<usize> = None;
            for (i, item) in list.iter().enumerate() {
                if item.as_str() == Some("auto") {
                    auto_at.get_or_insert(slots.len());
                    continue;
                }
                if !item.is_object() {
                    p.warn(format!("source {}: expected an object or \"auto\"", i + 1));
                    continue;
                }
                let Some(s) = p.source(item, i + 1, &mut palette_next, poll) else { continue };
                taken.push(s.id.clone());
                if item.get("enabled").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                slots.push(Some(s));
            }
            let mut out: Vec<Source> = slots.into_iter().flatten().collect();
            if let Some(at) = auto_at {
                let found: Vec<Source> = discover(home, appdata)
                    .into_iter()
                    .filter(|s| !taken.contains(&s.id))
                    .map(|mut s| {
                        s.poll_seconds = poll;
                        s
                    })
                    .collect();
                let at = at.min(out.len());
                out.splice(at..at, found);
            }
            out
        }
    };
    let mut sources = sources;
    dedupe_ids(&mut sources);
    Config { offset_x, position, theme, appearance, sources, warnings: p.warnings }
}

/// Reads the settings file. A missing file means defaults; an unreadable or
/// invalid one means defaults plus a warning saying why.
pub fn load() -> Config {
    let path = config_path();
    let (value, problem) = match std::fs::read(&path) {
        Err(_) => (None, None),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            match serde_json::from_str::<Value>(&jsonc::strip(text)) {
                Ok(v) if v.is_object() => (Some(v), None),
                Ok(_) => (None, Some("the settings file must hold a JSON object; using defaults".to_owned())),
                Err(e) => (None, Some(format!("settings file error at line {}, column {}: {e}; using defaults", e.line(), e.column()))),
            }
        }
    };
    let mut cfg = from_json(value.as_ref(), &home(), &appdata());
    if let Some(p) = problem {
        cfg.warnings.insert(0, p);
    }
    cfg
}

/// Modification time of the settings file, to notice edits.
pub fn stamp() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}

/// Written by "Edit settings" when there is no settings file yet.
pub fn starter() -> String {
    STARTER.replace("{SCHEMA}", SCHEMA_URL)
}

const STARTER: &str = r#"{
  // Limitbar settings. Saved changes apply within a few seconds.
  // Every option: https://github.com/vsolano9/limitbar/blob/main/docs/configuration.md
  "$schema": "{SCHEMA}",

  // Seconds between usage checks (minimum 60).
  "poll_seconds": 120,

  // "auto" shows every built-in provider signed in on this PC: Claude Code
  // (each config directory), Codex, Grok Build and Cursor Agent. List a
  // provider yourself to change its label, colour or bars, or set
  // "enabled": false to hide it. Add http, command and file providers for
  // anything else (examples: docs/providers.md).
  "sources": [
    "auto"
  ],

  "appearance": {
    "show": "used",        // or "remaining"
    "warn_at": 75,         // amber from this percent used
    "critical_at": 90      // red from this percent used
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("limitbar-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn touch(root: &Path, files: &[&str]) {
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{}").unwrap();
        }
    }

    fn labels(s: &[Source]) -> Vec<&str> {
        s.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn discovers_accounts_codex_grok_cursor() {
        let root = temp_root("discover");
        let appdata = root.join("AppData");
        std::fs::create_dir_all(root.join(".claude-accountx")).unwrap();
        touch(
            &root,
            &[
                ".claude-account2/.credentials.json",
                ".claude-account1/.credentials.json",
                ".codex/auth.json",
                ".grok/auth.json",
                "AppData/Cursor/auth.json",
            ],
        );
        // SAFETY: tests that read these variables do not run concurrently with writers.
        unsafe {
            std::env::remove_var("CODEX_HOME");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
        }
        let s = discover(&root, &appdata);
        assert_eq!(labels(&s), ["C1", "C2", "CX", "GK", "CU"]);
        let ids: Vec<_> = s.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["claude1", "claude2", "codex", "grok", "cursor"]);
        assert_eq!(s[1].name, "Claude 2");
        assert_eq!(s[4].provider, Provider::Builtin { kind: Builtin::Cursor, dir: appdata.join("Cursor") });
        assert_eq!(s[0].bars.len(), 2);
        assert_eq!(s[3].bars[0].keys, [WEEKLY, MONTHLY, DAILY]);
        assert_eq!(s[2].url.as_deref(), Some("https://chatgpt.com/codex/settings/usage"));
        // Signed out of Grok and Cursor: neither is shown.
        std::fs::remove_file(root.join(".grok/auth.json")).unwrap();
        std::fs::remove_file(root.join("AppData/Cursor/auth.json")).unwrap();
        assert_eq!(discover(&root, &appdata).len(), 3);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discovers_default_and_named_claude_dirs() {
        let root = temp_root("named");
        let appdata = root.join("AppData");
        touch(&root, &[".claude/.credentials.json"]);
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        let s = discover(&root, &appdata);
        assert_eq!((s[0].label.as_str(), s[0].name.as_str(), s[0].id.as_str()), ("CL", "Claude", "claude"));
        touch(&root, &[".claude-work/.credentials.json"]);
        let s = discover(&root, &appdata);
        assert_eq!(labels(&s), ["C1", "C2"]);
        assert_eq!((s[1].name.as_str(), s[1].id.as_str()), ("Claude (work)", "claude-work"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn config_overrides_1_x() {
        let v: Value = serde_json::from_str(
            r#"{"poll_seconds":5,"offset_x":160,"sources":[
                {"kind":"claude","dir":"D:/x","label":"W","name":"Work Claude"},
                {"kind":"grok"},
                {"kind":"cursor","label":"CR"},
                {"kind":"nope","dir":"y"}]}"#,
        )
        .unwrap();
        let c = from_json(Some(&v), Path::new("Z:/home"), Path::new("Z:/roaming"));
        assert_eq!(c.sources[0].poll_seconds, MIN_POLL);
        assert_eq!(c.offset_x, 160);
        assert_eq!(c.sources.len(), 3);
        assert_eq!(c.warnings.len(), 1);
        assert_eq!(c.sources[0].id, "workclaude");
        assert_eq!(c.sources[0].provider, Provider::Builtin { kind: Builtin::Claude, dir: PathBuf::from("D:/x") });
        let g = &c.sources[1];
        assert_eq!((g.label.as_str(), g.name.as_str(), g.id.as_str()), ("GK", "Grok", "grok"));
        assert_eq!(g.provider, Provider::Builtin { kind: Builtin::Grok, dir: Path::new("Z:/home").join(".grok") });
        let cu = &c.sources[2];
        assert_eq!((cu.label.as_str(), cu.id.as_str()), ("CR", "cr"));
        assert_eq!(cu.color, Builtin::Cursor.color());
    }

    #[test]
    fn custom_providers_and_auto() {
        let root = temp_root("custom");
        let appdata = root.join("AppData");
        touch(&root, &[".codex/auth.json", ".grok/auth.json"]);
        unsafe {
            std::env::remove_var("CODEX_HOME");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
        }
        let text = r##"{
            // comments are fine
            "sources": [
              {"provider":"http","name":"OpenRouter","label":"OR","url":"https://openrouter.ai/api/v1/credits",
               "vars":{"key":{"env":"OPENROUTER_API_KEY"}},
               "headers":{"Authorization":"Bearer ${key}"},
               "windows":[{"key":"credits","tag":"$","used":"/data/total_usage","limit":"/data/total_credits","unit":"USD"}]},
              "auto",
              {"provider":"grok","enabled":false},
              {"provider":"command","name":"Local","command":["pwsh","-File","~/x.ps1"],"color":"#112233","bars":["a",{"key":["b","c"],"tag":"bc"},"d","e"]},
              {"provider":"file","name":"Written","path":"~/usage.json","poll_seconds":1},
              {"provider":"http","name":"Broken"},
            ],
            "appearance":{"show":"remaining","bar_width":30,"colors":{"dark":{"warn":"#ff0"}}},
            "position":"right","theme":"light"
        }"##;
        let v: Value = serde_json::from_str(&jsonc::strip(text)).unwrap();
        let c = from_json(Some(&v), &root, &appdata);
        assert_eq!(labels(&c.sources), ["OR", "CX", "LO", "WR"]);
        let Provider::Http(h) = &c.sources[0].provider else { panic!() };
        assert_eq!(h.method, "GET");
        assert_eq!(h.vars, [("key".to_owned(), ValueSource::Env("OPENROUTER_API_KEY".into()))]);
        assert_eq!(c.sources[0].bars[0].tag.as_deref(), Some("$"));
        assert_eq!(c.sources[0].bars[0].unit.as_deref(), Some("USD"));
        assert_eq!(c.sources[0].color, PALETTE[0]);
        assert_eq!(c.sources[0].url, None);
        let local = &c.sources[2];
        assert_eq!(
            local.provider,
            Provider::Command { line: CommandLine::Argv(vec!["pwsh".into(), "-File".into(), "~/x.ps1".into()]), timeout: 30 }
        );
        assert_eq!(local.color.dark, rgba(0x11, 0x22, 0x33, 1.0));
        assert_eq!(local.bars.len(), MAX_BARS);
        assert_eq!(local.bars[1].keys, ["b", "c"]);
        let file = &c.sources[3];
        assert_eq!(file.provider, Provider::File { path: root.join("usage.json") });
        assert_eq!(file.poll_seconds, MIN_POLL_CUSTOM);
        assert_eq!(file.color, PALETTE[1]);
        assert!(file.bars.is_empty());
        assert!(c.appearance.show_remaining);
        assert_eq!(c.appearance.bar_width, 30.0);
        assert_eq!(c.appearance.dark.warn, rgba(255, 255, 0, 1.0));
        assert_eq!((c.position, c.theme), (Position::Right, ThemeMode::Light));
        // The broken http source and the extra bar are reported.
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_ids_are_suffixed() {
        let v: Value = serde_json::from_str(r#"{"sources":[{"provider":"codex"},{"provider":"codex","dir":"D:/other"}]}"#).unwrap();
        let c = from_json(Some(&v), Path::new("Z:/h"), Path::new("Z:/a"));
        let ids: Vec<_> = c.sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["codex", "codex-2"]);
    }

    #[test]
    fn colors_and_paths() {
        assert_eq!(parse_color("#E88E70"), Some(rgba(232, 142, 112, 1.0)));
        assert_eq!(parse_color("#00000080").unwrap().a, 128.0 / 255.0);
        assert_eq!(parse_color("#f80"), Some(rgba(255, 136, 0, 1.0)));
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("#12345"), None);
        let home = Path::new("C:/Users/me");
        assert_eq!(expand_path("~/.foo/auth.json", home), home.join(".foo/auth.json"));
        assert_eq!(expand_path("~", home), home.to_path_buf());
        assert_eq!(expand_path("~x", home), PathBuf::from("~x"));
        unsafe { std::env::set_var("LIMITBAR_TEST_DIR", "D:\\data") };
        assert_eq!(expand_path("%LIMITBAR_TEST_DIR%\\u.json", home), PathBuf::from("D:\\data\\u.json"));
        assert_eq!(expand_path("%NOT_A_LIMITBAR_VAR%\\u", home), PathBuf::from("%NOT_A_LIMITBAR_VAR%\\u"));
        assert_eq!(expand_path("50%", home), PathBuf::from("50%"));
    }

    #[test]
    fn example_file_parses_without_warnings() {
        let text = include_str!("../examples/config.example.jsonc");
        let v: Value = serde_json::from_str(&jsonc::strip(text)).unwrap();
        let c = from_json(Some(&v), Path::new("Z:/nobody"), Path::new("Z:/nobody/a"));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(labels(&c.sources), ["GPT", "CW", "GH", "OR", "C:", "CI"]);
    }

    #[test]
    fn starter_file_parses() {
        let v: Value = serde_json::from_str(&jsonc::strip(&starter())).unwrap();
        let c = from_json(Some(&v), Path::new("Z:/nobody"), Path::new("Z:/nobody/a"));
        assert!(c.warnings.is_empty());
        assert_eq!(v["$schema"], SCHEMA_URL);
    }
}
