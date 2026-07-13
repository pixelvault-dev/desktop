//! Persisted app state: the trial upload counter + the last few upload URLs.
//!
//! In v0 there is no sign-in yet, so crossing the free limit only *nudges*
//! (see `watcher`) — the real gate arrives with device login (slice 3).

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

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    uploads: u32,
    #[serde(default)]
    recent: Vec<String>,
    /// Upload signed-in images privately (signed URLs). Off by default.
    #[serde(default)]
    private_uploads: bool,
    /// Signed-URL lifetime, in seconds, for private uploads.
    #[serde(default = "default_sign_expires_secs")]
    sign_expires_secs: u64,
}

pub struct TrialState {
    path: PathBuf,
    uploads: AtomicU32,
    /// Most-recent-first, capped at `RECENT_LIMIT`.
    recent: Mutex<Vec<String>>,
    /// Whether signed-in uploads are made private (signed URLs).
    private_uploads: AtomicBool,
    /// Signed-URL lifetime (seconds) for private uploads, clamped to bounds.
    sign_expires_secs: AtomicU64,
}

impl TrialState {
    /// Load from `<config_dir>/state.json`, defaulting to empty.
    pub fn load(config_dir: PathBuf) -> Self {
        let path = config_dir.join("state.json");
        let persisted = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            uploads: AtomicU32::new(persisted.uploads),
            recent: Mutex::new(persisted.recent),
            private_uploads: AtomicBool::new(persisted.private_uploads),
            sign_expires_secs: AtomicU64::new(clamp_sign_expires(persisted.sign_expires_secs)),
        }
    }

    pub fn uploads(&self) -> u32 {
        self.uploads.load(Ordering::Relaxed)
    }

    pub fn remaining(&self) -> u32 {
        FREE_UPLOAD_LIMIT.saturating_sub(self.uploads())
    }

    pub fn recent(&self) -> Vec<String> {
        self.recent.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Whether signed-in uploads should be made private (signed URLs).
    pub fn private_uploads(&self) -> bool {
        self.private_uploads.load(Ordering::Relaxed)
    }

    /// Signed-URL lifetime (seconds) for private uploads.
    pub fn sign_expires_secs(&self) -> u64 {
        self.sign_expires_secs.load(Ordering::Relaxed)
    }

    /// Update the private-uploads preference and persist it.
    pub fn set_private_uploads(&self, on: bool) {
        self.private_uploads.store(on, Ordering::Relaxed);
        self.persist(self.uploads(), self.recent());
    }

    /// Update the signed-URL lifetime (clamped to bounds) and persist it.
    pub fn set_sign_expires_secs(&self, secs: u64) {
        self.sign_expires_secs
            .store(clamp_sign_expires(secs), Ordering::Relaxed);
        self.persist(self.uploads(), self.recent());
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

    /// Commit a reserved anonymous upload: push the URL to recent and persist.
    /// (The counter was already incremented by `try_reserve`.)
    pub fn commit_reserved(&self, url: &str) {
        let recent = self.push_recent_inner(url);
        self.persist(self.uploads(), recent);
    }

    /// Push a URL to the recent list WITHOUT touching the trial counter — used
    /// for keyed (signed-in) uploads, which aren't part of the free trial.
    pub fn push_recent(&self, url: &str) {
        let recent = self.push_recent_inner(url);
        self.persist(self.uploads(), recent);
    }

    fn push_recent_inner(&self, url: &str) -> Vec<String> {
        let mut r = self.recent.lock().unwrap();
        r.retain(|u| u != url); // de-dupe if the same URL recurs
        r.insert(0, url.to_string());
        r.truncate(RECENT_LIMIT);
        r.clone()
    }

    fn persist(&self, uploads: u32, recent: Vec<String>) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let snapshot = Persisted {
            uploads,
            recent,
            private_uploads: self.private_uploads(),
            sign_expires_secs: self.sign_expires_secs(),
        };
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = fs::write(&self.path, json);
        }
    }
}

/// Clamp a signed-URL lifetime to the range the API accepts, so a stale or
/// hand-edited value can never produce a request the server would reject.
fn clamp_sign_expires(secs: u64) -> u64 {
    secs.clamp(MIN_SIGN_EXPIRES_SECS, MAX_SIGN_EXPIRES_SECS)
}
