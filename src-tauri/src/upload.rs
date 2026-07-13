//! PNG encoding + upload to the PixelVault API.
//!
//! - No key → the **keyless** (anonymous) path: temporary (~30-day) upload.
//! - With a key → a **keyed** upload; pass `expires_in` for an ephemeral one.
//! Response envelope: `{ "data": { "url": ... } }`.

use std::io::Cursor;
use std::time::Duration;

use serde::Deserialize;

use crate::config::api_base;

/// Upload failure. `Unauthorized` (401/403) is separated so the caller can clear
/// a revoked/expired key and prompt re-sign-in. Messages are user-safe (never
/// the raw response body).
pub enum UploadError {
    Unauthorized,
    Failed(String),
}

impl UploadError {
    pub fn message(&self) -> String {
        match self {
            UploadError::Unauthorized => "Your session has expired.".to_string(),
            UploadError::Failed(m) => m.clone(),
        }
    }
}

#[derive(Deserialize)]
struct UploadEnvelope {
    data: UploadData,
}

#[derive(Deserialize)]
struct UploadData {
    url: String,
}

/// Encode raw RGBA8 bytes (as delivered by the clipboard) to a PNG byte vector.
pub fn encode_png(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "clipboard image buffer size mismatch".to_string())?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("png encode failed: {e}"))?;
    Ok(out.into_inner())
}

/// Options for a keyed (signed-in) upload.
#[derive(Default)]
pub struct KeyedOptions {
    /// Ephemeral image TTL in seconds (when the image itself auto-archives).
    pub expires_in: Option<u64>,
    /// Upload privately (mint a signed URL). Requires a secret key.
    pub private: bool,
    /// Signed-URL lifetime in seconds when `private` (the pasted link's TTL).
    pub sign_expires_in: Option<u64>,
}

/// Upload a PNG and return the hosted URL.
///
/// - `api_key`: `Some` → keyed; `None` → anonymous temporary (always public).
/// - `opts`: keyed-only knobs (image TTL, private/signed URL). Ignored when
///   anonymous — the server ignores `visibility` without a secret key anyway.
///
/// For a private upload the returned URL is the **signed** URL, ready to paste.
pub fn upload_png(
    png: Vec<u8>,
    api_key: Option<&str>,
    opts: KeyedOptions,
) -> Result<String, UploadError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| UploadError::Failed(e.to_string()))?;

    let part = reqwest::blocking::multipart::Part::bytes(png)
        .file_name("clipboard.png")
        .mime_str("image/png")
        .map_err(|e| UploadError::Failed(e.to_string()))?;
    let mut form = reqwest::blocking::multipart::Form::new().part("file", part);
    if api_key.is_some() {
        if let Some(secs) = opts.expires_in {
            form = form.text("expires_in", secs.to_string());
        }
        if opts.private {
            form = form.text("visibility", "private");
            if let Some(secs) = opts.sign_expires_in {
                form = form.text("sign_expires_in", secs.to_string());
            }
        }
    }

    let mut req = client.post(format!("{}/v1/images", api_base())).multipart(form);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().map_err(|e| UploadError::Failed(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(UploadError::Unauthorized);
        }
        // Friendly, body-free messages for the common cases.
        let msg = match status.as_u16() {
            402 => {
                "Private image limit reached — upgrade for unlimited private images.".to_string()
            }
            413 => "Image is too large.".to_string(),
            429 => "Rate limit reached — try again shortly.".to_string(),
            s => format!("Upload failed ({s})."),
        };
        return Err(UploadError::Failed(msg));
    }

    let env: UploadEnvelope = resp
        .json()
        .map_err(|e| UploadError::Failed(format!("bad response: {e}")))?;
    Ok(env.data.url)
}
