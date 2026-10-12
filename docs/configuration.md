# Configuration

Limitbar works without any settings: it shows every built-in provider it finds
signed in on your PC. It can also show this PC's own CPU, memory, GPU, disk and
network load (a [`system` source](providers.md#system)). To change what it shows or how it looks, right-click the
widget and choose **Edit settings…**. That opens
`%APPDATA%\Limitbar\config.json`, creating a starter file the first time.

Saved changes apply within a few seconds; there is no need to restart.
Problems in the file (a typo, an unknown provider) are listed at the bottom of
every tooltip and by `limitbar --probe`, and the rest of the file still applies.

The file is JSON with comments: `//` and `/* */` comments and trailing commas
are allowed. Add the schema line to get completion and validation in VS Code
and other editors:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/vsolano9/limitbar/main/limitbar.schema.json"
}
```

## Top-level options

| Option | Default | Meaning |
|---|---|---|
| `sources` | `["auto"]` (all signed-in built-ins) | What to show, in order. See [Sources](#sources). |
| `poll_seconds` | `120` | Default seconds between checks. Minimum 60. |
| `position` | `"auto"` | `"auto"`: left end of a centred taskbar, next to the clock when Start is on the left. `"left"` or `"right"` (just before the notification area) to force a side. |
| `offset_x` | `0` | Extra logical pixels from that edge, e.g. `200` to clear the Widgets (weather) button. Negative values move it the other way. |
| `theme` | `"auto"` | `"auto"` follows the Windows taskbar colour; `"dark"` or `"light"` force one palette. |
| `appearance` | | See [Appearance](#appearance). |

## Sources

`sources` is a list. Each entry is either the string `"auto"` or an object
describing one source.

`"auto"` adds every built-in provider that is signed in on this PC, at that
position in the list:

- **Claude Code**: `~\.claude` (or `CLAUDE_CONFIG_DIR`) and every
  `~\.claude-<name>` directory that holds a login. Several logins are shown as
  `C1`, `C2`, … (`~\.claude-account<N>` is always `C<N>`).
- **Codex**: `%CODEX_HOME%` or `~\.codex`
- **Grok Build**: `~\.grok`
- **Cursor Agent**: `%APPDATA%\Cursor`

A provider you list yourself is not added again by `"auto"`, so you can list
one to restyle it and keep `"auto"` for the rest. Set `"enabled": false` to hide
one:

```jsonc
"sources": [
  { "provider": "codex", "label": "GPT", "color": "#10A37F" },
  "auto",
  { "provider": "grok", "enabled": false }
]
```

Without `"auto"`, only the sources you list are shown.

### Options for every source

| Option | Meaning |
|---|---|
| `provider` | Required. `claude`, `codex`, `grok`, `cursor`, `http`, `command`, `file` or `system`. (`kind` is accepted too.) |
| `label` | Short tag on the taskbar. Built-ins default to `CL`, `CX`, `GK`, `CU`, system sources to `PC`; custom sources to the first two letters of `name`. |
| `name` | Name at the top of the tooltip. |
| `id` | Key for the cached values. Defaults to `name` in lower case without spaces; change it only to keep history when renaming. |
| `color` | Accent colour: `"#RRGGBB"` (also `#RGB`, `#RRGGBBAA`), or `{ "dark": "#…", "light": "#…" }` for a different shade on a light taskbar. Custom sources get a colour from a built-in palette. |
| `bars` | Which windows to draw as rows, top to bottom (at most 3; the tooltip lists all). See [Bars](#bars). |
| `page` | Web page for the right-click menu ("Open … usage page"). Built-ins link to their provider's usage page; system sources add **Open Task Manager**. |
| `poll_seconds` | Seconds between checks for this source. Minimum 60 for built-ins, 15 for custom providers. System sources sample every 2 seconds by default (1 to 60). |
| `enabled` | `false` hides the source. |

Options for specific providers:

| Provider | Option | Meaning |
|---|---|---|
| `claude`, `codex`, `grok`, `cursor` | `dir` | Where that CLI keeps its sign-in, if not the default. `~` and `%VARIABLES%` are expanded. |
| `http` | `url`, `method`, `headers`, `body`, `vars`, `expires`, `windows`, `plan`, `account`, `note` | See [providers.md](providers.md#http). |
| `command` | `command`, `timeout_seconds` | See [providers.md](providers.md#command). |
| `file` | `path` | See [providers.md](providers.md#file). |
| `system` | `top` | Busiest processes listed per metric in the tooltip (default 3, `0` for none). See [providers.md](providers.md#system). |

### Bars

Every provider reports one or more *windows*: a limit with a percentage used
and, usually, a reset time. Each has a key:

| Key | Tag | Where it comes from |
|---|---|---|
| `five_hour` | `5h` | Claude, Codex (plans with a 5-hour window) |
| `daily` | `day` | Grok (when the server reports a daily period) |
| `weekly` | `wk` | Claude, Codex, Grok |
| `monthly` | `mo` | Grok (when the server reports a monthly period) |
| `auto` | `cur` | Cursor: Cursor models (Auto + Composer) |
| `api` | `api` | Cursor: other models (named third-party models) |
| `cpu` | `cpu` | System: processor time |
| `memory` | `mem` | System: memory in use |
| `gpu` | `gpu` | System: busiest GPU engine |
| `disk` | `disk` | System: disk active time |
| `network` | `net` | System: busiest network adapter |

Custom providers use whatever keys they define.

`bars` picks and orders the rows. Each entry is a key, or an object with
`key` (a key, or a list of keys: the first one present is drawn) and optional
`tag`, `name` and `unit` overrides:

```jsonc
{ "provider": "claude", "bars": ["weekly"] }                       // weekly only
{ "provider": "claude", "bars": [{ "key": "weekly", "tag": "7d" }, "five_hour"] }
```

Built-ins default to their usual rows (Claude: `5h` and `wk`; Codex: `wk`;
Grok: its credit period; Cursor: `cur` and `api`; system: `cpu`, `mem` and
`gpu`). `http` sources default to
their `windows`; `command` and `file` sources without `bars` draw the first
three windows they report.

## Appearance

```jsonc
"appearance": {
  "font": "Segoe UI Variable Text",
  "font_size": 12,       // points at 100% scaling; row tags are one smaller
  "bar_width": 44,       // logical pixels
  "bar_height": 4,
  "gap": 18,             // space between sources
  "show_tags": true,     // the 5h / wk column
  "show_percent": true,  // the 42% column
  "show": "used",        // or "remaining": 100% means untouched
  "warn_at": 75,         // amber from this percent used
  "critical_at": 90,     // red from this percent used
  "colors": {
    "dark":  { "text": "#FFFFFFF2", "dim": "#FFFFFF94", "track": "#FFFFFF29", "warn": "#F7B955", "critical": "#FF6B61" },
    "light": { "text": "#000000E6", "dim": "#0000008F", "track": "#0000001F", "warn": "#B07000", "critical": "#C8281E" }
  }
}
```

Every key is optional; the values above are the defaults. The warning colours
always follow the percentage *used*, also in `"remaining"` mode.

## Files

| Path | What |
|---|---|
| `%APPDATA%\Limitbar\config.json` | Settings (yours; never written by Limitbar except to create the starter file). |
| `%LOCALAPPDATA%\Limitbar\state.json` | Last values per source, so the widget shows numbers immediately after a restart. No secrets. System load is never stored. |
| `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Limitbar` | Present when **Start with Windows** is checked. |

## Command line

```
limitbar            start the widget (or refresh the running one)
limitbar --probe    check every source once and print the results
limitbar --exit     stop the running widget
limitbar --version  print the version
```

`--probe` prints the settings file in use, any warnings, and one line per
source with every window, reset time, plan and provider note. It never prints
tokens or account names, does not touch the cache or a running widget, and
exits with code 1 if a source failed or the settings have problems. Providers
rate-limit their usage endpoints per sign-in, so do not run it in a loop while
the widget is also polling.
