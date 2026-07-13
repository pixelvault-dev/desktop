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
    // Drop any signed (private) URLs from the tray — the session that could
    // mint them is gone.
    app.state::<AppState>().trial.forget_private_recent();
    refresh_recent(app);
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

/// Render a duration in whole days/hours for a human-facing message. The signed
/// URL lifetimes we use are all whole days or hours (see the UI picker).
fn human_duration(secs: u64) -> String {
    let plural = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if secs % 86_400 == 0 {
        plural(secs / 86_400, "day")
    } else if secs % 3_600 == 0 {
        plural(secs / 3_600, "hour")
    } else if secs % 60 == 0 {
        plural(secs / 60, "minute")
    } else {
        plural(secs, "second")
    }
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
        // Honour the private-uploads preference (signed URLs) from settings.
        Some(key) => {
            let (private, sign_secs) = {
                let trial = &app.state::<AppState>().trial;
                (trial.private_uploads(), trial.sign_expires_secs())
            };
            let opts = upload::KeyedOptions {
                expires_in: Some(EPHEMERAL_SECS),
                private,
                sign_expires_in: private.then_some(sign_secs),
            };
            match upload::upload_png(png_bytes, Some(&key), opts) {
                Ok(url) => {
                    app.state::<AppState>().trial.push_recent(&url, private);
                    refresh_recent(app);
                    // The signed URL is a bearer capability; keep it off the
                    // notification (Notification Center history / lock screen).
                    // It's already on the clipboard + in the tray for this
                    // session.
                    if private {
                        notify(
                            app,
                            "Private link copied",
                            &format!("Paste to share · link expires in {}", human_duration(sign_secs)),
                        );
                    } else {
                        notify(app, "Image URL copied", &url);
                    }
                    Ok(Some(url))
                }
                // Revoked/expired key — clear the session and prompt re-sign-in.
                Err(upload::UploadError::Unauthorized) => {
                    handle_unauthorized(app);
                    Ok(None)
                }
                Err(e) => Err(e.message()),
            }
        }
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
            match upload::upload_png(png_bytes, None, upload::KeyedOptions::default()) {
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
    let recent = app.state::<AppState>().trial.recent_entries();
    let items = app
        .state::<AppState>()
        .recent_items
        .lock()
        .ok()
        .map(|g| g.clone());
    let Some(items) = items else { return };

    let updates: Vec<(String, bool)> = (0..items.len())
        .map(|i| match recent.get(i) {
            Some((url, private)) => (short_label(url, *private), true),
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

/// Label a recent upload for the tray by its final path segment, e.g.
/// `anon_l4f8nipug8ic.png`. The query string is dropped so a signed URL's token
/// never appears in the menu; private links get a lock marker.
fn short_label(url: &str, private: bool) -> String {
    let last = url.rsplit('/').next().unwrap_or(url);
    let name = last.split('?').next().unwrap_or(last);
    if private {
        format!("🔒 {name}")
    } else {
        name.to_string()
    }
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
    // Forget signed (private) URLs so a capability link doesn't linger post-sign-out.
    app.state::<AppState>().trial.forget_private_recent();
    refresh_recent(&app);
    refresh_account(&app);
    Ok(())
}

#[derive(serde::Serialize)]
struct Settings {
    private_uploads: bool,
    sign_expires_secs: u64,
}

fn read_settings(app: &AppHandle) -> Settings {
    let trial = &app.state::<AppState>().trial;
    Settings {
        private_uploads: trial.private_uploads(),
        sign_expires_secs: trial.sign_expires_secs(),
    }
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    read_settings(&app)
}

/// Persist the upload settings. Returns the stored (clamped) values so the UI
/// reflects exactly what was saved.
#[tauri::command]
fn set_settings(app: AppHandle, private_uploads: bool, sign_expires_secs: u64) -> Settings {
    app.state::<AppState>()
        .trial
        .set_upload_prefs(private_uploads, sign_expires_secs);
    read_settings(&app)
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
            sign_out,
            get_settings,
            set_settings
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

#[cfg(test)]
mod tests {
    use super::{human_duration, short_label};

    #[test]
    fn short_label_strips_the_signing_token() {
        // A signed (private) URL's token must never reach the tray label.
        let signed = "https://img.pixelvault.dev/proj/cp/i/img_abc.png?token=SECRET&expires=123";
        assert_eq!(short_label(signed, true), "🔒 img_abc.png");
        assert!(!short_label(signed, true).contains("SECRET"));
    }

    #[test]
    fn short_label_public_is_plain_filename() {
        assert_eq!(
            short_label("https://img.pixelvault.dev/proj/anon_xyz.png", false),
            "anon_xyz.png"
        );
    }

    #[test]
    fn human_duration_reads_naturally() {
        assert_eq!(human_duration(7 * 24 * 60 * 60), "7 days");
        assert_eq!(human_duration(24 * 60 * 60), "1 day");
        assert_eq!(human_duration(3600), "1 hour");
        assert_eq!(human_duration(60), "1 minute");
    }
}
