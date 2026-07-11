//! Persisted app state: the trial upload counter + the last few upload URLs.
//!
//! In v0 there is no sign-in yet, so crossing the free limit only *nudges*
//! (see `watcher`) — the real gate arrives with device login (slice 3).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Number of anonymous uploads before we prompt the user to sign in.
pub const FREE_UPLOAD_LIMIT: u32 = 5;

/// How many recent upload URLs to remember (and show in the tray).
pub const RECENT_LIMIT: usize = 5;

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    uploads: u32,
    #[serde(default)]
    recent: Vec<String>,
}

pub struct TrialState {
    path: PathBuf,
    uploads: AtomicU32,
    /// Most-recent-first, capped at `RECENT_LIMIT`.
    recent: Mutex<Vec<String>>,
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
        if let Ok(json) = serde_json::to_string(&Persisted { uploads, recent }) {
            let _ = fs::write(&self.path, json);
        }
    }
}
