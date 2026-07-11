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
  managed `AppState`, builds the tray, spawns the watcher, registers the Mode B
  hotkey. Also the **shared `upload_and_notify()` pipeline** used by both modes,
  plus `refresh_counter` / `refresh_recent` / `set_busy` (tray updates).
- `watcher.rs` — **Mode A (passive):** background thread polls the clipboard
  (`arboard`) every 1.2s, hashes the image to dedupe, and on a new image runs the
  shared pipeline, then writes the URL back to the clipboard.
- `capture.rs` — **Mode B (active):** global hotkey **⇧⌘2** → spawns a thread that
  runs macOS's native `/usr/sbin/screencapture -i <tempfile>`, uploads the PNG,
  and writes the URL to the clipboard.
- `auth.rs` — sign-in via `/v1/auth/device/{start,complete}`; stores the API key
  in the OS keychain (`keyring`). `config.rs` holds the shared API base.
- `upload.rs` — RGBA→PNG encode + multipart `POST /v1/images` (keyed with a
  Bearer key + `expires_in` when signed in, else anonymous); parses the
  `{ "data": { "url": ... } }` envelope. Base URL from `PIXELVAULT_API_BASE`.
- `state.rs` — `TrialState`: the free-upload counter (limit 5) + last 5 upload
  URLs, persisted to `<app_config_dir>/state.json`.
- `tray.rs` — the menu-bar icon (transparent template glyph) and menu (status,
  free-uploads counter, "Recent uploads" click-to-copy list, pause/resume,
  settings, quit).

## Key rules & gotchas

- **Mode B captures to a temp FILE, not the clipboard** — deliberately, so the
  passive watcher can't also see it and double-upload. Don't "simplify" it to
  `screencapture -c`.
- **Tray/menu mutations must run on the main thread** — always wrap
  `set_text` / `set_enabled` / `set_title` in `app.run_on_main_thread(...)`
  (AppKit requirement on macOS). The watcher/capture run on background threads.
- **`Image::from_bytes` needs the `image-png` Cargo feature** on `tauri` (already
  enabled) — that's how the tray icon is decoded from `include_bytes!`.
- **The keyless upload path needs no auth.** With no API key, uploads are
  anonymous + temporary (~30-day expiry). Sign-in (permanent uploads) is slice 3.
- **Trial gate is a soft nudge in v0** — there's no sign-in yet to unblock it, so
  crossing the 5-upload limit only notifies; it keeps uploading. Make it a real
  gate only alongside the sign-in flow.
- **`state.json` format is backward-compatible** — new fields use serde defaults;
  keep it that way so existing installs don't lose their counter.

## CI

`.github/workflows/build.yml` builds macOS (universal) and Linux on every push to
`main` and every PR, and attaches bundles to a GitHub Release on `v*` tags.
Binaries are **unsigned** (no Apple/Developer certs) — expect a Gatekeeper prompt.
