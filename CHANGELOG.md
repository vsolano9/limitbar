# Changelog

## Unreleased

- `install.ps1` starts Limitbar through Explorer, so the widget keeps running after the terminal or agent tool that ran the script closes. Some hosts run scripts in a job object that ends every child process with it.

## 2.1.0 - 2026-10-12

- **This PC's load**: a new `system` provider shows CPU, memory, GPU, disk and network use as Task Manager's Processes tab reports them, sampled every 2 seconds (`poll_seconds`, 1 to 60). It is optional: add `{ "provider": "system" }` to `sources` for a `PC` block with `cpu`, `mem` and `gpu`, or pick up to three metrics per source with `bars`.
  - Tooltips list the busiest processes per metric (`top`, default 3), grouped by name like Task Manager (`chrome (18) 1.4 GB`), plus details: used and total memory, the busiest GPU engine, disk throughput and the busiest disk, the network adapter and its link speed.
  - Only the configured metrics are measured, and system values are never cached. The menu gains **Open Task Manager**. The starter settings file includes a disabled `system` entry.
- Tooltip lines can carry extra details. System load reads "busy"/"idle" (memory "used"/"free") instead of "used"/"left".
- `--probe` measures system sources over one second and prints their process lists. It no longer panics when its output is cut short (`limitbar --probe | Select -First 3`).

## 2.0.0 - 2026-10-09

First open-source release (MIT).

- **Custom providers** without rebuilding:
  - `http`: one request to any JSON endpoint. Bars are mapped by JSON paths: a percentage (with scale), used or remaining against a limit, reset times as ISO 8601, unix seconds or ms, or seconds from now. Plan, account and note fields.
  - Secrets come from `vars`: environment variables, files (with a JSON path), Windows Credential Manager or a command. `${name}` and `${env:NAME}` templates work in the URL, headers and body.
  - Optional sign-in expiry: a timestamp or a JWT `exp`, after which the source shows as idle.
  - `command`: any program that prints Limitbar's provider format. It runs hidden, with a timeout and error reporting.
  - `file`: the same format read from a file another tool writes. The file's age counts as the time of the reading.
- **Any number of sources**, each polled on its own interval (`poll_seconds` per source), in parallel.
- **Settings**:
  - `config.json` accepts comments and trailing commas, has a JSON Schema (`limitbar.schema.json`) and reloads automatically when saved.
  - **Edit settings…** in the menu creates a starter file.
  - Problems are listed in every tooltip and by `--probe` instead of being ignored silently.
  - `"auto"` in `sources` adds every signed-in built-in provider at that position. Explicit entries override it, and `"enabled": false` hides one.
  - Per source: `color` (one colour, or dark and light), `bars` (choose, order and relabel rows, up to three), `page` (menu link), `poll_seconds`, `id`.
  - Appearance: font, size, bar width and height, gap, tags and percentages on or off, used or remaining, warning thresholds, palette colours for dark and light.
  - `position` (`auto`, `left`, `right`) and `theme` (`auto`, `dark`, `light`).
- **Discovery** finds `~\.claude` (or `CLAUDE_CONFIG_DIR`) and every `~\.claude-<name>` login, not only `~\.claude-account<N>`.
- Tooltips show amounts when a provider reports them ("12.5 of 50 USD"). The menu lists one usage page per distinct link.
- A placeholder block explains what to do when nothing is signed in or configured.
- `--version` and `--help`. `--probe` prints the settings file in use and its warnings.
- Internals:
  - each built-in provider is its own module under `src/providers/`;
  - the HTTP client supports `http://`, ports and concurrent requests;
  - the state cache keeps entries of hidden sources.
- The 1.x `kind`, `label`, `name` and `dir` settings and the 1.x cache still load.

## 1.1.0 - 2026-10-09

- Grok (Grok Build CLI) source `GK`:
  - weekly credit usage from `cli-chat-proxy.grok.com/v1/billing`, plan name from `/v1/settings`;
  - idle when the six-hour token has expired.
- Cursor (Cursor Agent CLI) source `CU`:
  - two monthly bars, Cursor models (`cur`) and other models (`api`), plan from `GetPlanInfo`;
  - Cursor's status line in the tooltip.
- Keyed usage windows per source; per-provider usage pages in the menu; `--probe`.

## 1.0.0 - 2026-10-08

- First release: taskbar widget with Claude 5-hour and weekly usage per account and Codex weekly usage, tooltips with reset times, click to refresh, dark/light theme, DPI aware, Explorer-restart recovery, cached values for idle accounts, installer.
