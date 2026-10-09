# Security

## How Limitbar handles credentials

- **Built-in providers** read the sign-in files the Claude Code, Codex, Grok
  Build and Cursor Agent CLIs already store, read-only. Limitbar never
  refreshes, rotates or writes them.
- **Custom providers** read secrets only through `vars`: environment variables,
  files, Windows Credential Manager or a command, at the moment a request is
  made. Prefer Credential Manager over plain files.
- Secrets are sent only in the request of the source that declares them, to
  the URL you configured. They are never written to `state.json`, logged, or
  shown in tooltips or `limitbar --probe` output.
- `state.json` holds only percentages, amounts, reset times, plan names,
  account names and provider notes.
- `command` sources run with your user rights. Only point them at scripts you
  trust, and keep `config.json` writable only by you (the default for
  `%APPDATA%`).

## Reporting a vulnerability

Please report security problems privately through
[GitHub's private vulnerability reporting](https://github.com/vsolano9/limitbar/security/advisories/new)
rather than in a public issue. Include the version (`limitbar --version`) and
steps to reproduce. You should get a reply within a week.
