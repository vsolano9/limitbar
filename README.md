# Limitbar

A tiny widget that lives inside the Windows 11 taskbar and shows how much of
your AI plan limits you have used: Claude, Codex, Grok and Cursor out of the
box, and any other provider, API or quota you add in a few lines of settings.

![Limitbar in the Windows 11 taskbar](docs/images/limitbar-taskbar.png)

*Claude, Codex, Grok and Cursor built in, plus two custom sources: the GitHub
API rate limit (`http`) and disk usage on C: (`command`).*

- **Zero setup** for Claude Code, Codex, Grok Build and Cursor Agent: it finds
  every signed-in CLI (and every Claude Code account) and shows its 5-hour,
  weekly or monthly windows.
- **Any provider, as many as you want**: point an `http` source at a JSON
  endpoint, run a script with a `command` source, or read a `file` another tool
  writes. No rebuilding, no plugins.
- **Yours to style**: labels, colours, which bars, used or remaining, fonts,
  sizes, thresholds, position. Settings reload as you save.
- **Tiny**: one 0.55 MB native exe (Rust, Win32, WinHTTP). No runtime, no
  browser engine. A few MB of memory and no CPU between checks.
- **Read-only with your sign-ins**: never refreshes, writes or logs a token.

Hover a block for the account, plan, exact numbers, reset times and when it
last updated. Click to refresh; right-click for provider pages, settings and
**Start with Windows**.

## Install

1. Download `limitbar.exe` from the
   [latest release](https://github.com/vsolano9/limitbar/releases/latest).
2. Put it somewhere permanent, for example
   `%LOCALAPPDATA%\Programs\Limitbar\limitbar.exe`, and run it.
3. Right-click the widget and check **Start with Windows**.

The exe is not code-signed, so SmartScreen may warn the first time: choose
*More info > Run anyway*, or build it yourself (below).

Requirements: Windows 11 (the widget docks into the Windows 11 taskbar).

To stop it, right-click > **Exit Limitbar** (or run `limitbar --exit`). To
uninstall, uncheck **Start with Windows**, exit, and delete the exe and the
`%APPDATA%\Limitbar` and `%LOCALAPPDATA%\Limitbar` folders.

### Build from source

Needs [Rust](https://rustup.rs) with the MSVC toolchain and the Windows SDK.

```powershell
git clone https://github.com/vsolano9/limitbar
cd limitbar
.\install.ps1             # build, install to %LOCALAPPDATA%\Programs\Limitbar, start, start with Windows
.\install.ps1 -NoStartup  # same, without the startup entry
.\install.ps1 -Exe .\limitbar.exe  # install a downloaded exe instead of building
.\install.ps1 -Uninstall  # stop and remove everything except your settings
```

## Add your own providers

Right-click the widget > **Edit settings…** opens
`%APPDATA%\Limitbar\config.json` (JSON with comments). Keep `"auto"` for the
built-ins and add whatever else you want:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/vsolano9/limitbar/main/limitbar.schema.json",
  "sources": [
    "auto",

    // Any JSON API: the key comes from Windows Credential Manager, the bar from the response.
    {
      "provider": "http", "name": "OpenRouter", "label": "OR",
      "url": "https://openrouter.ai/api/v1/credits",
      "vars": { "key": { "credential": "limitbar/openrouter" } },
      "headers": { "Authorization": "Bearer ${key}" },
      "windows": [ { "key": "credits", "tag": "$", "used": "/data/total_usage", "limit": "/data/total_credits", "unit": "USD" } ]
    },

    // Any script that prints {"windows":[{"used":..,"limit":..}]}.
    { "provider": "command", "name": "Drive C", "label": "C:",
      "command": ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "%USERPROFILE%\\limitbar\\disk-space.ps1"] }
  ],
  "appearance": { "show": "remaining", "warn_at": 80 }
}
```

- [docs/providers.md](docs/providers.md): the built-in providers, the `http`,
  `command` and `file` providers, and ready-to-paste examples (GitHub API rate
  limit, OpenRouter credits, extra Claude accounts, disk space, …).
- [docs/configuration.md](docs/configuration.md): every setting, the
  appearance options and the command line.

Check a configuration without waiting for the widget:

```
limitbar --probe
```

```
Settings: C:\Users\me\AppData\Roaming\Limitbar\config.json
C1   Claude 1     ok     5h 4.0% (resets 4:30 PM, in 3h 17m) | wk 89.0% (resets Tue 9:00 PM, in 4d 7h) | plan Max 20x
CX   Codex        ok     wk 100.0% (resets Wed 5:51 AM, in 4d 16h) | plan Pro
GH   GitHub API   ok     api 0.1% (resets 2:13 PM, in 1h 00m) | srch 0.0% (resets 1:14 PM, in 1m)
```

## How it works

- The widget is a layered child window of the taskbar (`Shell_TrayWnd`), so it
  hides with an auto-hiding taskbar and under full-screen apps, and re-attaches
  after Explorer restarts. It follows light/dark mode and per-monitor DPI, and
  docks next to the clock when the taskbar is left-aligned. Only the primary
  taskbar is used.
- Each source is a block of up to three bars; a bar turns amber at 75% and red
  at 90% (configurable). Values older than three check intervals fade.
- Every source is polled on its own interval (default 2 minutes), in parallel.
  Clicks refresh at most every 10 seconds. HTTP 429 pauses a source for at
  least five minutes.
- Last values are cached in `%LOCALAPPDATA%\Limitbar\state.json` (no secrets),
  so the widget shows numbers immediately after a restart and an idle account
  keeps its last values, rolling a window to 0% when its reset time passes.

## Privacy and security

Limitbar talks only to the endpoints of the sources you show. Built-in
providers read the CLIs' stored sign-ins read-only; custom providers read
secrets from environment variables, files, Windows Credential Manager or a
command at request time. Tokens are never written, cached or logged, and
`--probe` never prints tokens or account names. See [SECURITY.md](SECURITY.md).

Limitbar is not affiliated with Anthropic, OpenAI, xAI, Anysphere or any other
provider. The built-in providers use the same usage endpoints as the official
CLIs and apps; some are undocumented and may change.

## Contributing

Bug reports, provider definitions and pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). New built-in providers are one Rust module
each.

## License

[MIT](LICENSE)
