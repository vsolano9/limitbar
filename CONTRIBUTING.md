# Contributing

Thanks for helping. Limitbar stays small on purpose: one native exe, no
runtime, no background services. Changes that keep it that way are the easiest
to accept.

## Ways to help

- **Share a provider definition.** If you wrote an `http`, `command` or `file`
  source for a service, add it to the examples in
  [docs/providers.md](docs/providers.md) with a pull request (no secrets, and
  say which plan or API version you tested it with).
- **Report a bug** with `limitbar --version`, your Windows build, taskbar
  settings (alignment, auto-hide, scaling) and the output of `limitbar --probe`.
  The probe never prints tokens or account names, but check before posting.
- **Propose a built-in provider** for a service whose usage cannot be read with
  an `http` mapping (for example, it needs logic like reading a CLI's token
  store or classifying windows).

## Build and test

Requires Rust (MSVC toolchain, edition 2024) and the Windows SDK resource
compiler, on Windows.

```powershell
cargo test
cargo clippy --all-targets
cargo build --release      # target\release\limitbar.exe
.\install.ps1              # build and install for the current user
```

There is no CI; run the tests and clippy locally before opening a pull request.
`limitbar --probe` exercises every configured source against the real
endpoints; do not run it in a loop, the providers rate-limit per sign-in.

`tools/make_icon.py` (Python with Pillow) regenerates `res/limitbar.ico`.

## Code map

| Module | Responsibility |
|---|---|
| `main.rs` | Windows, layout, painting, tooltip, menu, settings reload, `--probe` |
| `display.rs` | Which rows a source draws, tags, names and tooltip lines (no Win32) |
| `config.rs` | Settings file, discovery of signed-in CLIs, defaults, warnings |
| `worker.rs` | Polling thread: per-source intervals, parallel checks, state cache |
| `providers/` | One module per built-in provider, plus `http`, `command`, `file` and `values` (secrets for custom providers) |
| `usage.rs` | The usage model, the provider output format, the cache format |
| `net.rs` | WinHTTP client |
| `jsonc.rs`, `jpath.rs` | Comments in settings, JSON paths |
| `render.rs` | Premultiplied software painter for the layered window |
| `sys.rs`, `time.rs` | Registry, startup entry, local time formatting, ISO 8601 |

## Adding a built-in provider

1. Create `src/providers/<name>.rs` with
   `pub fn fetch(http: &Http, dir: &Path, profile: &mut Profile) -> Result<Reading, Outcome>`.
   Read the stored sign-in read-only (never refresh or write it), return
   `Outcome::Idle` when it has expired, and keep the response parsing in a
   separate pure function with unit tests built from a real (trimmed,
   anonymised) response.
2. Use the well-known window keys in `usage.rs` (`five_hour`, `daily`, `weekly`,
   `monthly`) where they fit, or add a new key with its tag and name to `NAMES`.
3. In `config.rs`, add a `Builtin` variant and fill in its `key`, label, name,
   credential file, default directory, default bars, usage page and colours.
   Add it to discovery if it can be detected from a file.
4. Dispatch to it in `providers::fetch`.
5. Document it in the table in `docs/providers.md` and add it to
   `limitbar.schema.json` and the CHANGELOG.

Only call endpoints the provider's own CLI or web app uses for the signed-in
user, never paid APIs, and keep requests to one or two per check.

## Style

- `cargo fmt` (the repository's `rustfmt.toml` allows long lines) and no new
  clippy warnings.
- No new dependencies without a strong reason; `serde_json` and `windows-sys`
  are the only ones today.
- Comments explain why, not what. User-visible text is plain and specific.

## Releases

Bump the version in `Cargo.toml` and `res/limitbar.rc`, add a CHANGELOG entry,
then build the release exe with your local paths remapped (Rust embeds
dependency source paths in the binary):

```powershell
$env:RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE\.cargo=~/.cargo --remap-path-prefix=$env:USERPROFILE\.rustup=~/.rustup --remap-path-prefix=$PWD=."
cargo build --release
```

Attach `limitbar.exe` and its SHA-256 (`limitbar.exe.sha256`) to a GitHub
release tagged `v<version>`.
