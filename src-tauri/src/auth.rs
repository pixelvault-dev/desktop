//! Sign-in via the device-login endpoints + OS-native key storage.
//!
//! Flow: `device_start(email)` emails a 6-digit code; `device_complete(email,
//! code)` exchanges it for an app-scoped API key, stored in the OS keychain.

use std::time::Duration;

use serde::Deserialize;

use crate::config::api_base;

const KEYRING_SERVICE: &str = "dev.pixelvault.desktop";

fn entry(user: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, user).map_err(|e| e.to_string())
}

/// The stored API key, if signed in.
pub fn stored_key() -> Option<String> {
    entry("api_key").ok().and_then(|e| e.get_password().ok())
}

/// The signed-in email, if any.
pub fn stored_email() -> Option<String> {
    entry("email").ok().and_then(|e| e.get_password().ok())
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

pub fn sign_out() -> Result<(), String> {
    if let Ok(e) = entry("api_key") {
        let _ = e.delete_credential();
    }
    if let Ok(e) = entry("email") {
        let _ = e.delete_credential();
    }
    Ok(())
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

/// Exchange `code` for an API key and store it. Returns the signed-in email.
pub fn device_complete(email: &str, code: &str) -> Result<String, String> {
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
    Ok(env.data.email)
}
