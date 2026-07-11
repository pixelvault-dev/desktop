//! PixelVault desktop — menubar app.
//!
//! - Mode A (passive): watch the clipboard → keyless upload → URL on clipboard.
//! - Mode B (active): global hotkey → native `screencapture -i` → same pipeline.
//!
//! - Sign-in (device login) unlocks keyed uploads; signed-out uses the anonymous
//!   trial (5 free uploads).

mod auth;
mod capture;
mod config;
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
    /// The tray account status item ("Signed in as …" / "Not signed in").
    pub account_item: Mutex<Option<MenuItem<Wry>>>,
    /// The tray icon, so we can flash a busy title while uploading.
    pub tray_icon: Mutex<Option<TrayIcon<Wry>>>,
    /// Cached signed-in session. Loaded once from the keychain at startup and
    /// updated on sign-in/out, so uploads never hit the keychain (which could
    /// transiently fail and silently downgrade a signed-in user to anonymous).
    pub session: Mutex<Option<auth::Session>>,
    /// Whether the passive clipboard watcher is active.
    pub watching: AtomicBool,
}

/// The cached signed-in API key, if any.
fn current_key(app: &AppHandle) -> Option<String> {
    app.state::<AppState>()
        .session
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.api_key.clone()))
}

/// Whether a session is currently cached.
pub fn is_signed_in(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .session
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Clear the session (revoked/expired key) and prompt re-sign-in.
fn handle_unauthorized(app: &AppHandle) {
    let _ = auth::sign_out();
    if let Ok(mut g) = app.state::<AppState>().session.lock() {
        *g = None;
    }
    refresh_account(app);
    notify(
        app,
        "Session expired",
        "Please sign in again (Account & Settings).",
    );
    open_settings(app);
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

/// Ephemeral TTL applied to signed-in (keyed) uploads: 30 days.
const EPHEMERAL_SECS: u64 = 30 * 24 * 60 * 60;

/// Shared Mode A/B pipeline: upload PNG bytes → record + refresh tray → notify.
/// Returns `Some(url)` on upload (caller places it on the clipboard), or `None`
/// when the free trial is exhausted and the upload was gated (sign-in prompted).
pub fn upload_and_notify(app: &AppHandle, png_bytes: Vec<u8>) -> Result<Option<String>, String> {
    set_busy(app, true);
    let result = run_upload(app, png_bytes);
    set_busy(app, false);
    result
}

fn run_upload(app: &AppHandle, png_bytes: Vec<u8>) -> Result<Option<String>, String> {
    match current_key(app) {
        // Signed in → keyed, ephemeral (30d) upload; not part of the free trial.
        Some(key) => match upload::upload_png(png_bytes, Some(&key), Some(EPHEMERAL_SECS)) {
            Ok(url) => {
                app.state::<AppState>().trial.push_recent(&url);
                refresh_recent(app);
                notify(app, "Image URL copied", &url);
                Ok(Some(url))
            }
            // Revoked/expired key — clear the session and prompt re-sign-in.
            Err(upload::UploadError::Unauthorized) => {
                handle_unauthorized(app);
                Ok(None)
            }
            Err(e) => Err(e.message()),
        },
        // Signed out → anonymous trial. Reserve a slot atomically; if the free
        // limit is reached, HARD-gate: stop and prompt sign-in (a trial that
        // never blocks converts no one). Bypassable by design (client-side).
        None => {
            if !app.state::<AppState>().trial.try_reserve() {
                notify(
                    app,
                    "Sign in to keep uploading",
                    "You've used your 5 free uploads. Sign in (Account & Settings) for unlimited uploads + history.",
                );
                open_settings(app);
                return Ok(None);
            }
            match upload::upload_png(png_bytes, None, None) {
                Ok(url) => {
                    app.state::<AppState>().trial.commit_reserved(&url);
                    refresh_counter(app);
                    refresh_recent(app);
                    let remaining = app.state::<AppState>().trial.remaining();
                    notify(
                        app,
                        "Image URL copied",
                        &format!("{url}\n{remaining} of {} free uploads left", state::FREE_UPLOAD_LIMIT),
                    );
                    Ok(Some(url))
                }
                Err(e) => {
                    // Upload failed — release the reserved slot so it isn't burned.
                    app.state::<AppState>().trial.release();
                    Err(e.message())
                }
            }
        }
    }
}

/// Show + focus the settings/account window. Window ops run on the main thread
/// (this is called from the background watcher/capture threads on the gate path).
fn open_settings(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.set_focus();
        }
    });
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

/// Refresh the tray account status item from the cached session.
pub fn refresh_account(app: &AppHandle) {
    let email = app
        .state::<AppState>()
        .session
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.email.clone()));
    let item = app
        .state::<AppState>()
        .account_item
        .lock()
        .ok()
        .and_then(|g| g.clone());
    if let Some(item) = item {
        let text = match email {
            Some(email) => format!("Signed in as {email}"),
            None => "Not signed in".to_string(),
        };
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

// ---- Tauri commands (invoked from the settings window) ----

#[tauri::command]
fn sign_in_start(email: String) -> Result<(), String> {
    auth::device_start(email.trim())
}

#[tauri::command]
fn sign_in_complete(app: AppHandle, email: String, code: String) -> Result<String, String> {
    let session = auth::device_complete(email.trim(), code.trim())?;
    let email = session.email.clone();
    if let Ok(mut g) = app.state::<AppState>().session.lock() {
        *g = Some(session);
    }
    refresh_account(&app);
    Ok(email)
}

#[derive(serde::Serialize)]
struct AuthStatus {
    signed_in: bool,
    email: Option<String>,
    remaining: u32,
}

#[tauri::command]
fn auth_status(app: AppHandle) -> AuthStatus {
    let email = app
        .state::<AppState>()
        .session
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.email.clone()));
    AuthStatus {
        signed_in: email.is_some(),
        email,
        remaining: app.state::<AppState>().trial.remaining(),
    }
}

#[tauri::command]
fn sign_out(app: AppHandle) -> Result<(), String> {
    auth::sign_out()?;
    if let Ok(mut g) = app.state::<AppState>().session.lock() {
        *g = None;
    }
    refresh_account(&app);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            sign_in_start,
            sign_in_complete,
            auth_status,
            sign_out
        ])
        .setup(|app| {
            // Menubar-first: no dock icon on macOS.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));

            // Load the session once at startup. A keychain access error → treat
            // as signed out (log it) rather than crashing; uploads then use the
            // anonymous trial instead of a broken key.
            let session = auth::load_session().unwrap_or_else(|e| {
                eprintln!("[pixelvault] keychain read failed at startup: {e}");
                None
            });

            app.manage(AppState {
                trial: state::TrialState::load(config_dir),
                counter_item: Mutex::new(None),
                toggle_item: Mutex::new(None),
                recent_items: Mutex::new(Vec::new()),
                account_item: Mutex::new(None),
                tray_icon: Mutex::new(None),
                session: Mutex::new(session),
                watching: AtomicBool::new(true),
            });

            tray::build(app.handle())?;
            refresh_account(app.handle());
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
