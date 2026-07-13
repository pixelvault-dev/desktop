//! Persisted app state: the trial upload counter, the last few upload URLs, and
//! the private-uploads preference.
//!
//! In v0 there is no sign-in yet for the trial gate, so crossing the free limit
//! only *nudges* (see `watcher`).
//!
//! Private (signed) upload URLs are time-bounded **bearer capabilities**, so
//! they are kept in memory for the tray's click-to-copy but are NEVER written to
//! `state.json` (see `persist`) and are dropped from memory on sign-out. Public
//! URLs are harmless to persist and behave as before.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Number of anonymous uploads before we prompt the user to sign in.
pub const FREE_UPLOAD_LIMIT: u32 = 5;

/// How many recent upload URLs to remember (and show in the tray).
pub const RECENT_LIMIT: usize = 5;

/// Default signed-URL lifetime for private uploads: 7 days.
pub const DEFAULT_SIGN_EXPIRES_SECS: u64 = 7 * 24 * 60 * 60;
/// Bounds the API enforces on `sign_expires_in` (60s .. 30d). We clamp locally
/// so a stale/hand-edited state file can't push an out-of-range value.
pub const MIN_SIGN_EXPIRES_SECS: u64 = 60;
pub const MAX_SIGN_EXPIRES_SECS: u64 = 30 * 24 * 60 * 60;

fn default_sign_expires_secs() -> u64 {
    DEFAULT_SIGN_EXPIRES_SECS
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    uploads: u32,
    /// PUBLIC upload URLs only — private signed URLs are deliberately excluded.
    #[serde(default)]
    recent: Vec<String>,
    /// Upload signed-in images privately (signed URLs). Off by default.
    #[serde(default)]
    private_uploads: bool,
    /// Signed-URL lifetime, in seconds, for private uploads.
    #[serde(default = "default_sign_expires_secs")]
    sign_expires_secs: u64,
}

// A manual `Default` (not derived) so the no-state-file case matches the serde
// field defaults — in particular `sign_expires_secs` must default to 7 days, not
// 0. `#[serde(default = …)]` only governs deserialization, so a derived `Default`
// would silently yield a 0-second (→ clamped 60s) link TTL on a fresh install.
impl Default for Persisted {
    fn default() -> Self {
        Self {
            uploads: 0,
            recent: Vec::new(),
            private_uploads: false,
            sign_expires_secs: DEFAULT_SIGN_EXPIRES_SECS,
        }
    }
}

/// An entry in the in-memory recent list. `private` marks a signed (capability)
/// URL that must not be written to disk.
#[derive(Clone)]
struct RecentEntry {
    url: String,
    private: bool,
}

pub struct TrialState {
    path: PathBuf,
    uploads: AtomicU32,
    /// Most-recent-first, capped at `RECENT_LIMIT`.
    recent: Mutex<Vec<RecentEntry>>,
    /// Whether signed-in uploads are made private (signed URLs).
    private_uploads: AtomicBool,
    /// Signed-URL lifetime (seconds) for private uploads, clamped to bounds.
    sign_expires_secs: AtomicU64,
    /// Serializes the whole snapshot-and-write in `persist` so concurrent
    /// writers (watcher/hotkey uploads vs. a settings save) can't tear the file
    /// or lose a field.
    save_lock: Mutex<()>,
}

impl TrialState {
    /// Load from `<config_dir>/state.json`, defaulting to empty.
    pub fn load(config_dir: PathBuf) -> Self {
        let path = config_dir.join("state.json");
        let persisted = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .unwrap_or_default();
        // Everything on disk is public (private URLs are never persisted).
        let recent = persisted
            .recent
            .into_iter()
            .map(|url| RecentEntry {
                url,
                private: false,
            })
            .collect();
        Self {
            path,
            uploads: AtomicU32::new(persisted.uploads),
            recent: Mutex::new(recent),
            private_uploads: AtomicBool::new(persisted.private_uploads),
            sign_expires_secs: AtomicU64::new(clamp_sign_expires(persisted.sign_expires_secs)),
            save_lock: Mutex::new(()),
        }
    }

    pub fn uploads(&self) -> u32 {
        self.uploads.load(Ordering::Relaxed)
    }

    pub fn remaining(&self) -> u32 {
        FREE_UPLOAD_LIMIT.saturating_sub(self.uploads())
    }

    /// URLs for the tray (most-recent-first), including in-session private ones
    /// so click-to-copy still works before they're forgotten on sign-out.
    pub fn recent(&self) -> Vec<String> {
        self.recent
            .lock()
            .map(|r| r.iter().map(|e| e.url.clone()).collect())
            .unwrap_or_default()
    }

    /// Recent entries as `(url, is_private)`, for callers that must treat a
    /// private (signed, bearer) URL differently — e.g. keeping it out of a
    /// notification body or the menu label.
    pub fn recent_entries(&self) -> Vec<(String, bool)> {
        self.recent
            .lock()
            .map(|r| r.iter().map(|e| (e.url.clone(), e.private)).collect())
            .unwrap_or_default()
    }

    /// Whether signed-in uploads should be made private (signed URLs).
    pub fn private_uploads(&self) -> bool {
        self.private_uploads.load(Ordering::Relaxed)
    }

    /// Signed-URL lifetime (seconds) for private uploads.
    pub fn sign_expires_secs(&self) -> u64 {
        self.sign_expires_secs.load(Ordering::Relaxed)
    }

    /// Update both upload preferences (clamping the lifetime) and persist once.
    pub fn set_upload_prefs(&self, private_uploads: bool, sign_expires_secs: u64) {
        self.private_uploads.store(private_uploads, Ordering::Relaxed);
        self.sign_expires_secs
            .store(clamp_sign_expires(sign_expires_secs), Ordering::Relaxed);
        self.persist();
    }

    /// Atomically reserve one anonymous-trial slot. Returns `true` if a slot was
    /// available (counter incremented), `false` if the free limit is reached.
    /// A compare-and-swap loop makes admission atomic across the watcher and
    /// hotkey-capture threads, so two concurrent uploads can't both slip past
    /// the last free slot.
    pub fn try_reserve(&self) -> bool {
        let mut current = self.uploads.load(Ordering::Relaxed);
        loop {
            if current >= FREE_UPLOAD_LIMIT {
                return false;
            }
            match self.uploads.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    /// Release a reserved slot (upload failed after `try_reserve`).
    pub fn release(&self) {
        loop {
            let current = self.uploads.load(Ordering::Relaxed);
            let next = current.saturating_sub(1);
            if self
                .uploads
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Commit a reserved anonymous (always-public) upload: record the URL and
    /// persist. (The counter was already incremented by `try_reserve`.)
    pub fn commit_reserved(&self, url: &str) {
        self.push_recent_entry(url, false);
        self.persist();
    }

    /// Record a keyed (signed-in) upload URL. Private signed URLs are kept in
    /// memory for the tray but excluded from `state.json` by `persist`.
    pub fn push_recent(&self, url: &str, private: bool) {
        self.push_recent_entry(url, private);
        self.persist();
    }

    /// Drop private (signed) URLs from the in-memory recent list — called on
    /// sign-out / session loss so a capability URL doesn't linger in the tray.
    pub fn forget_private_recent(&self) {
        if let Ok(mut r) = self.recent.lock() {
            r.retain(|e| !e.private);
        }
        self.persist();
    }

    fn push_recent_entry(&self, url: &str, private: bool) {
        if let Ok(mut r) = self.recent.lock() {
            r.retain(|e| e.url != url); // de-dupe if the same URL recurs
            r.insert(
                0,
                RecentEntry {
                    url: url.to_string(),
                    private,
                },
            );
            r.truncate(RECENT_LIMIT);
        }
    }

    /// Snapshot all state and write it atomically. `save_lock` serializes the
    /// whole read-and-write so concurrent callers can't tear the file or lose a
    /// field; private URLs are filtered out so signed capabilities never touch
    /// the disk.
    fn persist(&self) {
        let _guard = self.save_lock.lock().unwrap_or_else(|e| e.into_inner());
        let recent = self
            .recent
            .lock()
            .map(|r| {
                r.iter()
                    .filter(|e| !e.private)
                    .map(|e| e.url.clone())
                    .collect()
            })
            .unwrap_or_default();
        let snapshot = Persisted {
            uploads: self.uploads(),
            recent,
            private_uploads: self.private_uploads(),
            sign_expires_secs: self.sign_expires_secs(),
        };
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(json) = serde_json::to_string(&snapshot) else {
            return;
        };
        // Write to a temp file then rename, so a concurrent or partial write
        // can't leave a truncated state.json (which `load` would silently reset
        // to defaults). `rename` is atomic on the same filesystem.
        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            // Owner-only: state.json holds upload history + prefs, so keep it out
            // of other local users' reach. Set on the temp file before the
            // rename so the final file is never briefly world-readable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
            }
            let _ = fs::rename(&tmp, &self.path);
        }
    }
}

/// Clamp a signed-URL lifetime to the range the API accepts, so a stale or
/// hand-edited value can never produce a request the server would reject.
fn clamp_sign_expires(secs: u64) -> u64 {
    secs.clamp(MIN_SIGN_EXPIRES_SECS, MAX_SIGN_EXPIRES_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory per call (no external deps).
    fn temp_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pv-desktop-test-{}-{}", std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clamp_covers_bounds_inclusive() {
        assert_eq!(clamp_sign_expires(0), MIN_SIGN_EXPIRES_SECS);
        assert_eq!(clamp_sign_expires(59), MIN_SIGN_EXPIRES_SECS);
        assert_eq!(clamp_sign_expires(60), 60);
        assert_eq!(clamp_sign_expires(604_800), 604_800);
        assert_eq!(clamp_sign_expires(MAX_SIGN_EXPIRES_SECS), MAX_SIGN_EXPIRES_SECS);
        assert_eq!(clamp_sign_expires(u64::MAX), MAX_SIGN_EXPIRES_SECS);
    }

    #[test]
    fn defaults_when_state_absent() {
        let s = TrialState::load(temp_dir());
        assert!(!s.private_uploads());
        assert_eq!(s.sign_expires_secs(), DEFAULT_SIGN_EXPIRES_SECS);
        assert!(s.recent().is_empty());
    }

    #[test]
    fn prefs_round_trip_and_clamp_on_set() {
        let dir = temp_dir();
        {
            let s = TrialState::load(dir.clone());
            s.set_upload_prefs(true, 5); // below MIN → clamps to 60
        }
        let s = TrialState::load(dir);
        assert!(s.private_uploads());
        assert_eq!(s.sign_expires_secs(), MIN_SIGN_EXPIRES_SECS);
    }

    #[test]
    fn private_urls_are_never_persisted_but_public_are() {
        let dir = temp_dir();
        {
            let s = TrialState::load(dir.clone());
            s.commit_reserved("https://img/pub.png"); // public
            s.push_recent("https://img/cp/i/secret.png?sig=abc", true); // private
            assert_eq!(s.recent().len(), 2, "both are visible in-memory for the tray");
        }
        // Reloaded from disk: the signed (private) URL must be gone.
        let s = TrialState::load(dir);
        assert_eq!(s.recent(), vec!["https://img/pub.png".to_string()]);
    }

    #[test]
    fn forget_private_recent_drops_only_private() {
        let s = TrialState::load(temp_dir());
        s.push_recent("https://img/pub.png", false);
        s.push_recent("https://img/priv.png?sig=x", true);
        assert_eq!(s.recent().len(), 2);
        s.forget_private_recent();
        assert_eq!(s.recent(), vec!["https://img/pub.png".to_string()]);
    }

    #[test]
    fn loads_old_state_without_new_fields() {
        let dir = temp_dir();
        fs::write(
            dir.join("state.json"),
            r#"{"uploads":3,"recent":["https://img/a.png"]}"#,
        )
        .unwrap();
        let s = TrialState::load(dir);
        assert_eq!(s.uploads(), 3);
        assert_eq!(s.recent(), vec!["https://img/a.png".to_string()]);
        assert!(!s.private_uploads());
        assert_eq!(s.sign_expires_secs(), DEFAULT_SIGN_EXPIRES_SECS);
    }
}
