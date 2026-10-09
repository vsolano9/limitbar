# Providers

A *source* is one block on the taskbar. Its *provider* is where the numbers
come from. Limitbar has four built-in providers and three generic ones that
let you add any service, script or file without rebuilding anything.

- [Built-in providers](#built-in-providers): Claude, Codex, Grok, Cursor
- [`http`](#http): call any JSON endpoint and map fields to bars
- [`command`](#command): run any program that prints Limitbar's JSON
- [`file`](#file): read Limitbar's JSON from a file another tool writes
- [Examples](#examples)
- [Adding a built-in provider](../CONTRIBUTING.md#adding-a-built-in-provider)

You can add as many sources as you like, mixing providers and several accounts
of the same provider. Each source is polled on its own interval, in parallel.

## Built-in providers

The built-ins read the sign-in each CLI already stores and call the same usage
endpoint the CLI or its web app uses. They never refresh or write those
credentials: OAuth refresh rotates the refresh token, which would sign the CLI
out. When a stored token has expired the account is idle, so its usage cannot
have grown; Limitbar keeps showing the last values and drops a window to 0%
once its reset time passes. The next time you use that CLI it refreshes its own
token and Limitbar picks it up.

The Grok and Cursor endpoints are undocumented, so every field is parsed
defensively. Limitbar is not affiliated with any of these companies; an
endpoint can change without notice.

| Provider | Windows | Credentials | Endpoint |
|---|---|---|---|
| `claude` (Claude Code, Pro/Max sign-in) | `five_hour`, `weekly` | `<dir>\.credentials.json` (`claudeAiOauth.accessToken`); account from `<dir>\.claude.json` | `GET https://api.anthropic.com/api/oauth/usage` |
| `codex` (Codex CLI, ChatGPT sign-in) | `weekly`, plus `five_hour` on plans that have one | `<dir>\auth.json` (`tokens.access_token`, `tokens.account_id`) | `GET https://chatgpt.com/backend-api/wham/usage` |
| `grok` (Grok Build CLI) | the credit period: `weekly`, `monthly` or `daily` | `<dir>\auth.json`, first `https://auth.x.ai::<id>` entry | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`; plan from `/v1/settings` |
| `cursor` (Cursor Agent CLI) | `auto` (Cursor models), `api` (other models) | `<dir>\auth.json` (`accessToken`); account from `~\.cursor\cli-config.json` | `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage`; plan from `GetPlanInfo` |

API-key logins (for example `codex` with an OpenAI API key) have no plan
limits, so there is nothing for these providers to show. Use an `http` source
for API credit balances instead.

## http

One HTTP(S) request to a JSON endpoint per check. You describe the request and
where the numbers are in the response.

```jsonc
{
  "provider": "http",
  "name": "OpenRouter",
  "label": "OR",
  "url": "https://openrouter.ai/api/v1/credits",
  "vars": { "key": { "credential": "limitbar/openrouter" } },
  "headers": { "Authorization": "Bearer ${key}" },
  "windows": [
    { "key": "credits", "tag": "$", "name": "Credits", "used": "/data/total_usage", "limit": "/data/total_credits", "unit": "USD" }
  ],
  "page": "https://openrouter.ai/settings/credits"
}
```

| Option | Meaning |
|---|---|
| `url` | Required. `https://…` or `http://…` (any port, e.g. a local proxy). |
| `method` | `GET` by default, `POST` when there is a `body`. |
| `headers` | Object of header names to values. `Accept: application/json` is added unless you set it. |
| `body` | A string, or any JSON value (sent as JSON with `Content-Type: application/json` unless you set one). |
| `vars` | Named values substituted as `${name}` into `url`, `headers` and `body`. See [Variables](#variables). |
| `expires` | When the sign-in expires. Past it, the source is idle (shows its last values) instead of failing, and HTTP 401/403 also counts as idle. `{ "jwt": "token" }` reads the `exp` claim of the JWT in var `token`; any [variable](#variables) form (for example `{ "file": "…", "json": "/expiresAt" }`) reads a timestamp. |
| `windows` | Required. One entry per limit; see below. |
| `plan`, `account`, `note` | JSON paths to text for the tooltip header (plan, account) and an extra status line (note). |

`${env:NAME}` reads an environment variable without declaring it in `vars`.

### Windows

Each window needs a percentage, or an amount used or remaining together with a
limit. Values are [JSON paths](#json-paths) into the response; `limit` can
also be a plain number.

| Option | Meaning |
|---|---|
| `key` | Name of the window (`w1`, `w2`, … if omitted). Use a [well-known key](configuration.md#bars) to get its usual tag and name. |
| `tag`, `name`, `unit` | Display: tag beside the bar, name in the tooltip, unit after the amounts ("12.5 of 50 USD"). |
| `percent` | Path to the percentage used. |
| `scale` | Multiplies `percent`. Use `100` when the API returns a fraction (0.42). |
| `used`, `remaining`, `limit` | Amounts; the percentage is `used / limit` or `(limit - remaining) / limit`. |
| `default` | Percentage to use when the fields are missing (protobuf JSON omits zeros). Without it, a window whose fields are missing is shown as `–`. |
| `resets_at` | Path to the reset time: ISO 8601 text, unix seconds or unix milliseconds. |
| `resets_in` | Path to the seconds until reset. |

HTTP 429 pauses the source for at least five minutes (or the `Retry-After`
time). Other errors show in the tooltip with the server's error message.

### JSON paths

Paths are [JSON Pointers](https://datatracker.ietf.org/doc/html/rfc6901):
`/data/usage`, `/items/0/percent` (array index), `/a~1b` for a key containing
`/`. One extension picks an element of an array of objects by a field:
`/limits[kind=session]/percent` is the `percent` of the first element of
`limits` whose `kind` is `session`.

### Variables

Secrets do not belong in `config.json`. `vars` read them when a request is
made; nothing is stored or logged.

| Form | Value |
|---|---|
| `{ "env": "NAME" }` | An environment variable. |
| `{ "file": "~/.tool/auth.json", "json": "/tokens/access" }` | One value in a JSON file (for example a CLI's stored sign-in). Without `json`, the file's whole trimmed text. `~` and `%VARIABLES%` are expanded. |
| `{ "credential": "limitbar/openrouter" }` | The password of a generic Windows Credential Manager entry. |
| `{ "command": "gh auth token" }` | A command's trimmed output (a string runs through `cmd.exe`; a list runs a program directly). |
| `"text"` | A literal, for values that are not secret (an organisation id). |

To store a key in Credential Manager:

```powershell
cmdkey /generic:limitbar/openrouter /user:limitbar /pass:sk-or-...
```

(or *Control Panel > Credential Manager > Windows Credentials > Add a generic
credential*). Remove it with `cmdkey /delete:limitbar/openrouter`.

## command

Runs a program on every check, hidden, and reads Limitbar's JSON from its
standard output. Use it for anything an `http` mapping cannot express: several
requests, logins, local data, other quotas entirely.

```jsonc
{
  "provider": "command",
  "name": "Drive C",
  "label": "C:",
  "command": ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "%USERPROFILE%\\limitbar\\disk-space.ps1", "-Drive", "C"],
  "poll_seconds": 600
}
```

| Option | Meaning |
|---|---|
| `command` | Required. A string runs through `cmd.exe /d /s /c`; a list runs the program with those arguments (`~` and `%VARIABLES%` in arguments are expanded). |
| `timeout_seconds` | Default 30. A command that runs longer is stopped and the check fails. |

A non-zero exit code fails the check and shows the first line of standard error.

### The provider format

```jsonc
{
  "windows": [
    {
      "key": "credits",          // optional, defaults to w1, w2, ...
      "tag": "$",                // optional display overrides
      "name": "API credits",
      "unit": "USD",
      "percent": 25,             // either a percentage used ...
      "used": 12.5,              // ... or used / remaining with a limit
      "remaining": 37.5,
      "limit": 50,
      "resets_at": "2026-11-01T00:00:00Z",   // ISO 8601, unix seconds or ms
      "resets_in": 3600                      // or seconds from now
    }
  ],
  "plan": "Pay as you go",       // optional, shown in the tooltip header
  "account": "me@example.com",   // optional
  "note": "Low balance",         // optional extra tooltip line
  "idle": "Signed out",          // optional: report idle (keep the last values)
  "error": "Could not log in"    // optional: report a failure
}
```

## file

Reads the same format from a file. Another program (a scheduled task, a
script, a service) writes it; Limitbar re-reads it every `poll_seconds`
(minimum 15). The file's modification time counts as the time of the reading,
so a file nobody updates fades out as stale after three intervals (at least
ten minutes).

```jsonc
{ "provider": "file", "name": "Build minutes", "label": "CI", "path": "%LOCALAPPDATA%\\ci-usage.json" }
```

Write the file atomically (write a temporary file, then rename it) so Limitbar
never reads half of it.

## Examples

### GitHub API rate limit

Uses the GitHub CLI's token, so it shows your authenticated limit (5,000
requests an hour):

```jsonc
{
  "provider": "http", "name": "GitHub API", "label": "GH",
  "url": "https://api.github.com/rate_limit",
  "vars": { "token": { "command": "gh auth token" } },
  "headers": { "Authorization": "Bearer ${token}", "User-Agent": "limitbar" },
  "windows": [
    { "key": "core", "tag": "api", "name": "REST API requests", "used": "/resources/core/used", "limit": "/resources/core/limit", "resets_at": "/resources/core/reset" },
    { "key": "search", "tag": "srch", "name": "Search requests", "used": "/resources/search/used", "limit": "/resources/search/limit", "resets_at": "/resources/search/reset" }
  ],
  "color": "#C9D1D9"
}
```

### OpenRouter credits

`/api/v1/credits` needs a *management* key
([docs](https://openrouter.ai/docs/api-reference/get-credits)); store it in
Credential Manager as shown above.

```jsonc
{
  "provider": "http", "name": "OpenRouter", "label": "OR",
  "url": "https://openrouter.ai/api/v1/credits",
  "vars": { "key": { "credential": "limitbar/openrouter" } },
  "headers": { "Authorization": "Bearer ${key}" },
  "windows": [ { "key": "credits", "tag": "$", "name": "Credits", "used": "/data/total_usage", "limit": "/data/total_credits", "unit": "USD" } ],
  "page": "https://openrouter.ai/settings/credits"
}
```

### A second Claude Code account in a custom directory

```jsonc
{ "provider": "claude", "dir": "D:\\profiles\\claude-work", "label": "CW", "name": "Claude (work)" }
```

### The Codex built-in, written as an http source

The built-ins are code, but most could be expressed as `http` sources. This is
the Codex provider as a mapping, a useful template for other OAuth CLIs:

```jsonc
{
  "provider": "http", "name": "Codex (http)", "label": "CX",
  "url": "https://chatgpt.com/backend-api/wham/usage",
  "vars": {
    "token":   { "file": "~/.codex/auth.json", "json": "/tokens/access_token" },
    "account": { "file": "~/.codex/auth.json", "json": "/tokens/account_id" }
  },
  "expires": { "jwt": "token" },
  "headers": { "Authorization": "Bearer ${token}", "ChatGPT-Account-Id": "${account}" },
  "windows": [ { "key": "weekly", "percent": "/rate_limit/primary_window/used_percent", "resets_at": "/rate_limit/primary_window/reset_at" } ],
  "plan": "/plan_type"
}
```

### Disk space (command)

[`examples/disk-space.ps1`](../examples/disk-space.ps1) reports how full a
drive is; copy it somewhere and point a `command` source at it (see the
example under [command](#command)).

### Anything a script can measure

```powershell
# my-quota.ps1: print Limitbar's format
$used = 42; $limit = 100   # look these up however you like
@{ windows = @(@{ key = 'quota'; used = $used; limit = $limit; unit = 'jobs' }) } | ConvertTo-Json -Depth 4 -Compress
```

Share a provider definition that works for you in
[Discussions](https://github.com/vsolano9/limitbar/discussions) or as a pull
request to this page.
