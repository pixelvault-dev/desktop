//! Background clipboard watcher (Mode A — passive).
//!
//! Polls the clipboard for a new image; on change, uploads it (shared pipeline)
//! and swaps the hosted URL onto the clipboard as text. Runs on its own OS
//! thread using `arboard`, so it never blocks the UI.

use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{upload, AppState};

/// How often to poll the clipboard.
const POLL_INTERVAL: Duration = Duration::from_millis(1200);

pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[pixelvault] clipboard init failed: {e}");
                return;
            }
        };

        // Hash of the last image we acted on, so we don't re-upload the same
        // content on every poll. (A deliberate re-copy of the *same* image
        // within a session won't re-upload — acceptable for v0.)
        let mut last_hash: Option<u64> = None;
        // Hash of an image that was gated (free trial used up). We keep it in the
        // clipboard and skip re-prompting for it while signed out — but once the
        // user signs in, we let it through so the exact image that triggered the
        // sign-in gets uploaded without needing a re-copy.
        let mut gated_hash: Option<u64> = None;

        loop {
            std::thread::sleep(POLL_INTERVAL);

            if !app.state::<AppState>().watching.load(Ordering::Relaxed) {
                continue;
            }

            // No image on the clipboard → nothing to do.
            let img = match clipboard.get_image() {
                Ok(img) => img,
                Err(_) => continue,
            };

            let hash = hash_image(&img);
            if Some(hash) == last_hash {
                continue;
            }
            // Previously gated this exact image and still signed out → don't
            // re-prompt every poll. (Once signed in, fall through and upload it.)
            if Some(hash) == gated_hash && !crate::is_signed_in(&app) {
                continue;
            }

            let width = img.width as u32;
            let height = img.height as u32;
            let rgba = img.bytes.into_owned();

            let png = match upload::encode_png(width, height, rgba) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[pixelvault] encode error: {e}");
                    last_hash = Some(hash);
                    continue;
                }
            };

            match crate::upload_and_notify(&app, png) {
                Ok(Some(url)) => {
                    last_hash = Some(hash);
                    gated_hash = None;
                    // Swap the URL onto the clipboard (replaces the image).
                    if let Err(e) = clipboard.set_text(url) {
                        eprintln!("[pixelvault] failed to set clipboard text: {e}");
                    }
                }
                // Gated (free trial used up) — sign-in was prompted. Remember the
                // hash (don't advance last_hash) so a subsequent sign-in retries
                // this exact image.
                Ok(None) => {
                    gated_hash = Some(hash);
                }
                Err(e) => {
                    last_hash = Some(hash);
                    eprintln!("[pixelvault] upload error: {e}");
                    crate::notify(&app, "Upload failed", &e);
                }
            }
        }
    });
}

fn hash_image(img: &arboard::ImageData) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    img.width.hash(&mut h);
    img.height.hash(&mut h);
    img.bytes.hash(&mut h);
    h.finish()
}
