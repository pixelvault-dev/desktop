# PixelVault Desktop

[![Release](https://img.shields.io/github/v/release/pixelvault-dev/desktop)](https://github.com/pixelvault-dev/desktop/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)
![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB)

A macOS/Linux **menubar app**: copy an image, and a hosted URL replaces it on
your clipboard — ready to paste into a coding agent that only accepts text
(Codex cloud, Claude Code on the web, Cursor background agents, Daytona/e2b
sandboxes). The remote side needs nothing installed; it just receives a URL.

## Install

**macOS** — [Homebrew](https://brew.sh):

```bash
brew install --cask pixelvault-dev/tap/pixelvault
```

Or download the latest **`PixelVault_*_universal.dmg`** from the
[Releases page](https://github.com/pixelvault-dev/desktop/releases/latest)
(one bundle for both Intel and Apple Silicon). Builds are currently **unsigned**,
so on first launch **right-click the app → Open** to get past Gatekeeper.

**Linux** — grab the latest **`.AppImage`**, **`.deb`**, or **`.rpm`** from
[Releases](https://github.com/pixelvault-dev/desktop/releases/latest). (macOS is
the primary target; Linux builds but is less battle-tested.)

## How it works

Two ways to get an image, one result — a hosted URL on your clipboard:

- **Mode A (passive watch):** a background thread watches the clipboard. When an
  **image** appears, it's uploaded and the **hosted URL** replaces it. On macOS,
  **⇧⌃⌘4** copies a screen region straight to the clipboard (note the `Ctrl` —
  plain ⇧⌘4 saves a file to the Desktop and won't be picked up).
- **Mode B (active hotkey):** press **⇧⌘2** — the app runs macOS's native region
  capture (`screencapture -i`, the same crosshair select), uploads the grab, and
  puts the URL on your clipboard.

Uploads take ~0.5–2s. The **"Image URL copied" notification is your cue** that the
clipboard now holds the URL — then **⌘V** to paste. A `⋯` on the menu-bar icon
means an upload is in flight.

## Accounts & privacy

- **Without signing in:** an anonymous free trial — **5 uploads**, after which the
  app prompts you to sign in. Links are public and **time-limited (~30 days)**.
- **Sign in** (device login — no password) for **keyed uploads with no trial
  gate**, a **Recent uploads** history in the tray menu, and optional **private,
  signed URLs** (a toggle in Settings — the app hands you a ready-to-paste signed
  link).

Links are time-limited by design — this is a *paste-into-your-agent* tool, not
long-term hosting. For permanent image hosting, use
[PixelVault](https://pixelvault.dev) directly (API, dashboard, and a free tier).

## Build from source

Prerequisites: [Rust](https://rustup.rs), Node 18+, and Xcode Command Line Tools
(macOS).

```bash
npm install
npm run tauri dev      # run the app
npm run tauri build    # produce a bundle (.dmg on macOS; .AppImage/.deb/.rpm on Linux)
```

Point at a non-production API while testing:

```bash
PIXELVAULT_API_BASE=https://api-staging.pixelvault.dev npm run tauri dev
```

## Stack

Tauri v2 (Rust core + system webview). Clipboard I/O via `arboard`, PNG encoding
via `image`, uploads via blocking `reqwest`. The tray menu is the primary UI; the
small window is a status/settings surface.

## License

MIT © PixelVault. See [LICENSE](./LICENSE).
