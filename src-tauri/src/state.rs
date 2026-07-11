//! Trial upload counter, persisted to the app config dir.
//!
//! In v0 there is no sign-in yet, so crossing the free limit only *nudges*
//! (see `watcher`) — the real gate arrives with device login (slice 3).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// Number of anonymous uploads before we prompt the user to sign in.
pub const FREE_UPLOAD_LIMIT: u32 = 5;

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    uploads: u32,
}

pub struct TrialState {
    path: PathBuf,
    uploads: AtomicU32,
}

impl TrialState {
    /// Load the counter from `<config_dir>/state.json`, defaulting to 0.
    pub fn load(config_dir: PathBuf) -> Self {
        let path = config_dir.join("state.json");
        let uploads = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .map(|p| p.uploads)
            .unwrap_or(0);
        Self {
            path,
            uploads: AtomicU32::new(uploads),
        }
    }

    pub fn uploads(&self) -> u32 {
        self.uploads.load(Ordering::Relaxed)
    }

    pub fn remaining(&self) -> u32 {
        FREE_UPLOAD_LIMIT.saturating_sub(self.uploads())
    }

    /// Increment and persist. Returns the new total.
    pub fn record_upload(&self) -> u32 {
        let n = self.uploads.fetch_add(1, Ordering::Relaxed) + 1;
        self.persist(n);
        n
    }

    fn persist(&self, uploads: u32) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&Persisted { uploads }) {
            let _ = fs::write(&self.path, json);
        }
    }
}
