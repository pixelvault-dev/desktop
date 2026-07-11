//! Sign-in via the device-login endpoints + OS-native key storage.
//!
//! Flow: `device_start(email)` emails a 6-digit code; `device_complete(email,
//! code)` exchanges it for an app-scoped API key, stored in the OS keychain.
//!
//! Keychain access **fails closed**: a real access error propagates as `Err`
//! rather than being flattened to "signed out", so a transient keychain failure
//! can't silently downgrade a signed-in user to anonymous uploads.

use std::time::Duration;

use serde::Deserialize;

use crate::config::api_base;

const KEYRING_SERVICE: &str = "dev.pixelvault.desktop";

/// A signed-in session. Both fields are always present together.
#[derive(Clone)]
pub struct Session {
    pub email: String,
    pub api_key: String,
}

fn entry(user: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, user).map_err(|e| e.to_string())
}

/// Read one keychain item, distinguishing "absent" (`Ok(None)`) from a real
/// access error (`Err`).
fn read(user: &str) -> Result<Option<String>, String> {
    match entry(user)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Delete one keychain item. "Not found" counts as success.
fn delete(user: &str) -> Result<(), String> {
    match entry(user)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Load the persisted session. Requires BOTH the key and the email; a real
/// access error propagates (fail closed). A half-written state (only one
/// present) is cleared and treated as signed out.
pub fn load_session() -> Result<Option<Session>, String> {
    let api_key = read("api_key")?;
    let email = read("email")?;
    match (api_key, email) {
        (Some(api_key), Some(email)) => Ok(Some(Session { email, api_key })),
        (None, None) => Ok(None),
        _ => {
            let _ = sign_out();
            Ok(None)
        }
    }
}

fn store(api_key: &str, email: &str) -> Result<(), String> {
    entry("api_key")?
        .set_password(api_key)
        .map_err(|e| e.to_string())?;
    entry("email")?
        .set_password(email)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete the stored session. Surfaces any real delete error so we never claim
/// a sign-out that actually left the API key behind. Attempts both regardless.
pub fn sign_out() -> Result<(), String> {
    let a = delete("api_key");
    let b = delete("email");
    a.and(b)
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct ErrEnvelope {
    error: ErrBody,
}
#[derive(Deserialize)]
struct ErrBody {
    message: String,
}

/// Pull the API's error message out of a JSON body, or fall back to the status.
fn error_message(body: &str, status: reqwest::StatusCode) -> String {
    serde_json::from_str::<ErrEnvelope>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| format!("Request failed ({status})"))
}

/// Request a sign-in code for `email`.
pub fn device_start(email: &str) -> Result<(), String> {
    let resp = client()?
        .post(format!("{}/v1/auth/device/start", api_base()))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().unwrap_or_default();
    Err(error_message(&body, status))
}

#[derive(Deserialize)]
struct CompleteEnvelope {
    data: CompleteData,
}
#[derive(Deserialize)]
struct CompleteData {
    api_key: String,
    email: String,
}

/// Exchange `code` for an API key, store it in the keychain, and return the
/// resulting session so the caller can cache it in memory.
pub fn device_complete(email: &str, code: &str) -> Result<Session, String> {
    let resp = client()?
        .post(format!("{}/v1/auth/device/complete", api_base()))
        .json(&serde_json::json!({ "email": email, "code": code }))
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(error_message(&body, status));
    }
    let env: CompleteEnvelope = resp.json().map_err(|e| format!("bad response: {e}"))?;
    store(&env.data.api_key, &env.data.email)?;
    Ok(Session {
        email: env.data.email,
        api_key: env.data.api_key,
    })
}
