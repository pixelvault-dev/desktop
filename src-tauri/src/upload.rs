//! PNG encoding + upload to the PixelVault API.
//!
//! - No key → the **keyless** (anonymous) path: temporary (~30-day) upload.
//! - With a key → a **keyed** upload; pass `expires_in` for an ephemeral one.
//! Response envelope: `{ "data": { "url": ... } }`.

use std::io::Cursor;
use std::time::Duration;

use serde::Deserialize;

use crate::config::api_base;

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

/// Upload a PNG and return the hosted URL.
///
/// - `api_key`: `Some` → keyed (permanent unless `expires_in` set); `None` →
///   anonymous temporary.
/// - `expires_in`: keyed ephemeral TTL in seconds (ignored when anonymous).
pub fn upload_png(
    png: Vec<u8>,
    api_key: Option<&str>,
    expires_in: Option<u64>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let part = reqwest::blocking::multipart::Part::bytes(png)
        .file_name("clipboard.png")
        .mime_str("image/png")
        .map_err(|e| e.to_string())?;
    let mut form = reqwest::blocking::multipart::Form::new().part("file", part);
    if api_key.is_some() {
        if let Some(secs) = expires_in {
            form = form.text("expires_in", secs.to_string());
        }
    }

    let mut req = client.post(format!("{}/v1/images", api_base())).multipart(form);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("upload failed ({status}): {body}"));
    }

    let env: UploadEnvelope = resp.json().map_err(|e| format!("bad response: {e}"))?;
    Ok(env.data.url)
}
