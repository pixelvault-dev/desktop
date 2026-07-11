# PixelVault Desktop

A macOS/Linux **menubar app**: copy an image, and a hosted URL replaces it on
your clipboard — ready to paste into a coding agent that only accepts text
(Codex cloud, Claude Code on the web, Cursor background agents, Daytona/e2b
sandboxes). The remote side needs nothing installed; it just receives a URL.

> **Status: v0 (slices 1–2).** Passive clipboard watch + active-hotkey capture →
> keyless upload → URL on clipboard. macOS-first. No sign-in yet (planned),
> Linux build pending. See `docs/specs/clipboard-desktop-app.md` in the main
> PixelVault repo.

## How it works

Two ways to get an image, one result — a hosted URL on your clipboard:

- **Mode A (passive watch):** a background thread watches the clipboard. When an
  **image** appears, it's uploaded and the **hosted URL** replaces it. On macOS,
  **⇧⌃⌘4** copies a screen region straight to the clipboard (note the `Ctrl` —
  plain ⇧⌘4 saves a file to the Desktop and won't be picked up).
- **Mode B (active hotkey):** press **⇧⌘2** — the app invokes macOS's native
  `screencapture -i` (the same crosshair region select), captures to a temp file,
  uploads it, and puts the URL on your clipboard.

In both modes the upload takes ~0.5–2s. The **"Image URL copied" notification is
your cue** that the clipboard now holds the URL — then **⌘V** to paste. While an
upload is in flight the menu-bar icon shows a `⋯` busy indicator.

Anonymous uploads are **temporary (~30 days)**. The tray shows your remaining
free uploads; sign-in for permanent uploads + history is coming.

## Develop

Prerequisites: [Rust](https://rustup.rs), Node 18+, and Xcode Command Line Tools
(macOS).

```bash
npm install
npm run tauri dev      # run the app
npm run tauri build    # produce a bundle
```

Point at a non-production API while testing:

```bash
PIXELVAULT_API_BASE=https://api-staging.pixelvault.dev npm run tauri dev
```

## Stack

Tauri v2 (Rust core + system webview). Clipboard I/O via `arboard`, PNG encoding
via `image`, uploads via blocking `reqwest`. The UI is the tray menu; the small
window is a status/settings surface.

## License

MIT © PixelVault. See [LICENSE](./LICENSE).
