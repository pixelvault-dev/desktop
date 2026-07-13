# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repository.

## Project Overview

PixelVault Desktop is a **macOS/Linux menu-bar app**: copy an image, and a hosted
URL replaces it on your clipboard — for feeding images to coding agents that only
accept text (Codex cloud, Claude Code on the web, Cursor background agents,
Daytona/e2b sandboxes). The remote side needs nothing installed; it just receives
a URL. This is a distribution/top-of-funnel client for **PixelVault**
(`api.pixelvault.dev`); the backend lives in the main `pixelvault` repo.

**Status:** Passive clipboard watch + active-hotkey capture → upload → URL on
clipboard, with **sign-in** (device login): signed-in users get keyed uploads
(ephemeral, 30d); signed-out users get the anonymous trial. macOS-first; Linux
builds but is less tested. Design specs live in the main repo:
`docs/specs/clipboard-desktop-app.md` and `docs/specs/device-login.md`.

## Stack

- **Tauri v2** — Rust core + the OS's system webview.
- **Rust** (`src-tauri/`) does the real work; the TypeScript/Vite frontend
  (`src/`) is only a small status/settings window. The tray menu is the primary UI.
- Clipboard I/O via `arboard`, PNG encoding via `image`, uploads via blocking
  `reqwest`.

## Commands

```bash
npm install                 # install frontend deps
npm run tauri dev           # run the app (compiles Rust, launches menubar)
npm run tauri build         # produce a bundle (.dmg / .app on macOS; .deb/.AppImage on Linux)
npm run build               # frontend only (tsc + vite) → dist/
(cd src-tauri && cargo check)   # fast Rust type-check
(cd src-tauri && cargo fmt)     # format Rust

# Regenerate app icons from a source PNG (>=1024px square):
npm run tauri icon <path-to-1024.png>
# NOTE: `tauri icon` also emits src-tauri/icons/{android,ios}/ — delete them,
# this app is macOS/Linux only.
```

Point at a non-production API while testing:

```bash
PIXELVAULT_API_BASE=https://api-staging.pixelvault.dev npm run tauri dev
```

## Architecture (`src-tauri/src/`)

- `lib.rs` — Tauri builder + `setup`: Accessory activation policy (no Dock icon),
  loads the cached session, manages `AppState`, builds the tray, spawns the
  watcher, registers the Mode B hotkey + the sign-in commands (`sign_in_start` /
  `sign_in_complete` / `auth_status` / `sign_out`) and the settings commands
  (`get_settings` / `set_settings`). Also the **shared `run_upload`
  pipeline** (`upload_and_notify`): it branches on the cached session — signed-in
  → keyed ephemeral upload (honouring the **private-uploads** preference → signed
  URL); signed-out → anonymous trial with an atomic reservation + hard gate. Plus
  `refresh_counter` / `refresh_recent` / `refresh_account` / `set_busy` (tray
  updates, all on the main thread).
- `watcher.rs` — **Mode A (passive):** background thread polls the clipboard
  (`arboard`) every 1.2s, hashes to dedupe, runs the shared pipeline, writes the
  URL back. Tracks a separate `gated_hash` so a gated image is retried once the
  user signs in, without re-prompting every poll while signed out.
- `capture.rs` — **Mode B (active):** global hotkey **⇧⌘2** → spawns a thread that
  runs macOS's native `/usr/sbin/screencapture -i <tempfile>`, uploads, writes URL.
- `auth.rs` — sign-in via `/v1/auth/device/{start,complete}`. A `Session {email,
  api_key}` is stored in the OS keychain (`keyring`); `load_session()` **fails
  closed** (a real keychain error propagates rather than flattening to "signed
  out"). `config.rs` holds the shared API base.
- `upload.rs` — RGBA→PNG encode + multipart `POST /v1/images`. Keyed = Bearer +
  `KeyedOptions` (`expires_in`; `visibility=private` + `sign_expires_in` when
  private — the server returns the ready-to-paste **signed** URL); else anonymous
  (always public). Returns `UploadError::Unauthorized` on 401/403 so the caller
  clears the session + re-prompts; a `402` maps to a "private image limit reached"
  message. Base from `PIXELVAULT_API_BASE`.
- `state.rs` — `TrialState`: free-upload counter (limit 5) + last 5 URLs + the
  **private-uploads** preference (`private_uploads` + `sign_expires_secs`, clamped
  to 60s..30d, default 7d) in `<app_config_dir>/state.json`. Anonymous admission
  is atomic: `try_reserve` (compare-exchange) → `commit_reserved` / `release`.
- `tray.rs` — menu-bar icon (transparent template glyph) + menu: status, account
  ("Signed in as …" / "Not signed in"), free-uploads counter, "Recent uploads"
  click-to-copy, pause/resume, Account & Settings…, quit.

## Key rules & gotchas

- **Mode B captures to a temp FILE, not the clipboard** — deliberately, so the
  passive watcher can't also see it and double-upload. Don't "simplify" it to
  `screencapture -c`.
- **Tray/menu mutations must run on the main thread** — always wrap
  `set_text` / `set_enabled` / `set_title` in `app.run_on_main_thread(...)`
  (AppKit requirement on macOS). The watcher/capture run on background threads.
- **`Image::from_bytes` needs the `image-png` Cargo feature** on `tauri` (already
  enabled) — that's how the tray icon is decoded from `include_bytes!`.
- **Auth is a cached, fail-closed session.** The API key is read once from the
  keychain at startup into `AppState.session` and used from there — never per
  upload (a transient keychain error must not silently downgrade a signed-in user
  to anonymous). `sign_out` surfaces delete errors; a 401/403 clears the session
  and re-prompts.
- **The trial gate is HARD.** Signed-out, after 5 free anonymous uploads, further
  uploads are blocked — `run_upload` returns `Ok(None)`, prompts sign-in, and puts
  no URL on the clipboard. Signed-in users are keyed + unlimited. It's client-side
  and bypassable by design; the server-side anonymous rate limiter is the real ceiling.
- **`upload_and_notify` returns `Result<Option<String>>`** — `Some(url)` uploaded,
  `None` gated (caller does nothing — no false "Upload failed"), `Err` real failure.
- **The keyless (no-key) upload path is anonymous + temporary (~30 days).**
- **`state.json` format is backward-compatible** — new fields use serde defaults;
  keep it that way so existing installs don't lose their counter.

## CI

`.github/workflows/build.yml` builds macOS (universal) + Linux on every push to
`main` and PR, and attaches bundles to a draft GitHub Release on `v*` tags.
Binaries are **unsigned** today (Gatekeeper prompt). Apple signing/notarization is
**wired but off** — set the repo variable `APPLE_SIGNING_ENABLED=true` + the six
`APPLE_*` secrets to enable it. (The vars must be *absent*, not empty, when off — an
empty `APPLE_CERTIFICATE` makes the Tauri bundler fail; hence the conditional
`Configure Apple signing` step.)

`.github/workflows/update-cask.yml` bumps the Homebrew cask on a published release
— disabled until `HOMEBREW_AUTO_BUMP=true` + a `HOMEBREW_TAP_TOKEN` PAT (with
`contents:write` on the tap repo) are set.

## Distribution

- **Releases:** push a `v*` tag → CI builds + attaches a draft GitHub Release;
  publish it manually.
- **Homebrew:** `brew install --cask pixelvault-dev/tap/pixelvault` (tap repo
  `pixelvault-dev/homebrew-tap`, `Casks/pixelvault.rb`). Bump `version` + `sha256`
  per release (automatable via `update-cask.yml`).
- **Landing page + comparison blog post:** `pixelvault.dev/desktop` and
  `/blog/images-into-cloud-coding-agents`, in the main `pixelvault` repo (`apps/web`).
- **In-agent skill:** `/pixelvault-desktop` in the `pixelvault-dev/skill` plugin.
