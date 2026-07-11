//! Mode B — active capture via a global hotkey.
//!
//! On the hotkey, we invoke macOS's native `screencapture -i` (the same
//! crosshair / drag-region UX as ⇧⌘4) to a **temp file** — deliberately NOT the
//! clipboard, so the passive watcher never sees it and can't double-upload. We
//! then upload the PNG and place the hosted URL on the clipboard.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

/// Absolute path so it works even when the app is launched with a minimal PATH.
const SCREENCAPTURE_BIN: &str = "/usr/sbin/screencapture";

/// Trigger an interactive region capture. Runs on its own thread: `screencapture`
/// blocks until the user finishes selecting, and the upload is network-bound, so
/// we keep both off the UI thread.
pub fn trigger(app: AppHandle) {
    std::thread::spawn(move || {
        let path = temp_path();

        let status = Command::new(SCREENCAPTURE_BIN)
            .arg("-i") // interactive region/window selection
            .arg(&path)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(_) => {
                eprintln!("[pixelvault] screencapture exited non-zero");
                return;
            }
            Err(e) => {
                crate::notify(
                    &app,
                    "Capture failed",
                    &format!("Could not run screencapture: {e}"),
                );
                return;
            }
        }

        // Cancelled (Esc) → no file was written; nothing to do.
        let bytes = match std::fs::read(&path) {
            Ok(b) if !b.is_empty() => b,
            _ => return,
        };
        let _ = std::fs::remove_file(&path);

        match crate::upload_and_notify(&app, bytes) {
            Ok(Some(url)) => {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(url);
                }
            }
            Ok(None) => {} // gated (free trial used up) — sign-in was prompted
            Err(e) => crate::notify(&app, "Upload failed", &e),
        }
    });
}

fn temp_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("pixelvault-capture-{nanos}.png"))
}
