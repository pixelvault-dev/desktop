//! PixelVault desktop — menubar app.
//!
//! - Mode A (passive): watch the clipboard → keyless upload → URL on clipboard.
//! - Mode B (active): global hotkey → native `screencapture -i` → same pipeline.
//!
//! No sign-in yet (slice 3).

mod capture;
mod state;
mod tray;
mod upload;
mod watcher;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::menu::MenuItem;
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Default Mode B hotkey. ⇧⌘3/4/5 are taken by macOS; ⇧⌘2 is free.
const CAPTURE_SHORTCUT: &str = "CmdOrCtrl+Shift+2";

/// App-wide shared state (managed by Tauri).
pub struct AppState {
    pub trial: state::TrialState,
    /// The tray "Free uploads left" item, updated after each upload.
    pub counter_item: Mutex<Option<MenuItem<Wry>>>,
    /// The tray "Pause/Resume watching" item.
    pub toggle_item: Mutex<Option<MenuItem<Wry>>>,
    /// The tray "Recent uploads" slots (fixed count), updated after each upload.
    pub recent_items: Mutex<Vec<MenuItem<Wry>>>,
    /// The tray icon, so we can flash a busy title while uploading.
    pub tray_icon: Mutex<Option<TrayIcon<Wry>>>,
    /// Whether the passive clipboard watcher is active.
    pub watching: AtomicBool,
}

/// Show a native notification. Safe to call from any thread.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

/// Shared Mode A/B pipeline: nudge if out of free uploads → upload PNG bytes →
/// record + refresh the counter → notify. Returns the hosted URL. Does NOT touch
/// the clipboard — the caller decides how to place the URL.
pub fn upload_and_notify(app: &AppHandle, png_bytes: Vec<u8>) -> Result<String, String> {
    set_busy(app, true);
    let result = run_upload(app, png_bytes);
    set_busy(app, false);
    result
}

fn run_upload(app: &AppHandle, png_bytes: Vec<u8>) -> Result<String, String> {
    // v0 nudge: no sign-in yet to unblock the gate, so we only warn and keep
    // uploading (the real ceiling is the server-side anonymous rate limiter).
    if app.state::<AppState>().trial.remaining() == 0 {
        notify(
            app,
            "Free uploads used up",
            "You've used your 5 free uploads. Sign-in is coming soon — still uploading for now.",
        );
    }

    let url = upload::upload_png(png_bytes)?;

    app.state::<AppState>().trial.record_upload(&url);
    refresh_counter(app);
    refresh_recent(app);

    let remaining = app.state::<AppState>().trial.remaining();
    notify(
        app,
        "Image URL copied",
        &format!("{url}\n{remaining} of {} free uploads left", state::FREE_UPLOAD_LIMIT),
    );
    Ok(url)
}

/// Flash the tray title while an upload is in flight (an always-visible "busy"
/// cue in the menu bar). Menu/tray mutations run on the main thread (AppKit).
fn set_busy(app: &AppHandle, busy: bool) {
    let tray = app
        .state::<AppState>()
        .tray_icon
        .lock()
        .ok()
        .and_then(|g| g.clone());
    if let Some(tray) = tray {
        let title: Option<String> = if busy { Some("⋯".to_string()) } else { None };
        let _ = app.run_on_main_thread(move || {
            let _ = tray.set_title(title);
        });
    }
}

/// Refresh the tray "Free uploads left" label from current trial state.
pub fn refresh_counter(app: &AppHandle) {
    let st = app.state::<AppState>();
    let remaining = st.trial.remaining();
    let item = st.counter_item.lock().ok().and_then(|g| g.clone());
    if let Some(item) = item {
        let text = format!("Free uploads left: {}/{}", remaining, state::FREE_UPLOAD_LIMIT);
        let _ = app.run_on_main_thread(move || {
            let _ = item.set_text(text);
        });
    }
}

/// Refresh the tray "Recent uploads" slots from persisted state. Filled slots
/// show the image filename and are clickable (copy the URL); empty slots show
/// "—" and are disabled.
pub fn refresh_recent(app: &AppHandle) {
    let recent = app.state::<AppState>().trial.recent();
    let items = app
        .state::<AppState>()
        .recent_items
        .lock()
        .ok()
        .map(|g| g.clone());
    let Some(items) = items else { return };

    let updates: Vec<(String, bool)> = (0..items.len())
        .map(|i| match recent.get(i) {
            Some(url) => (short_label(url), true),
            None => ("—".to_string(), false),
        })
        .collect();

    let _ = app.run_on_main_thread(move || {
        for (item, (text, enabled)) in items.iter().zip(updates) {
            let _ = item.set_text(text);
            let _ = item.set_enabled(enabled);
        }
    });
}

/// Label a URL by its final path segment, e.g. `anon_l4f8nipug8ic.png`.
fn short_label(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

/// Toggle the passive watcher on/off and update the tray label.
pub fn toggle_watching(app: &AppHandle) {
    let st = app.state::<AppState>();
    let now = !st.watching.load(Ordering::Relaxed);
    st.watching.store(now, Ordering::Relaxed);
    let label = if now { "Pause watching" } else { "Resume watching" };
    let item = st.toggle_item.lock().ok().and_then(|g| g.clone());
    if let Some(item) = item {
        let _ = app.run_on_main_thread(move || {
            let _ = item.set_text(label);
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // Menubar-first: no dock icon on macOS.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));

            app.manage(AppState {
                trial: state::TrialState::load(config_dir),
                counter_item: Mutex::new(None),
                toggle_item: Mutex::new(None),
                recent_items: Mutex::new(Vec::new()),
                tray_icon: Mutex::new(None),
                watching: AtomicBool::new(true),
            });

            tray::build(app.handle())?;
            watcher::spawn(app.handle().clone());

            // Mode B: register the global capture hotkey.
            app.global_shortcut()
                .on_shortcut(CAPTURE_SHORTCUT, |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        capture::trigger(app.clone());
                    }
                })?;

            // The settings window is hidden on launch (shown from the tray).
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.hide();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
