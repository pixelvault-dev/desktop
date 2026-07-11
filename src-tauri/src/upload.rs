//! PNG encoding + keyless upload to the PixelVault API.
//!
//! v0 uses the **keyless** (anonymous) path: `POST /v1/images` with no auth →
//! a temporary (~30-day) upload. Response envelope: `{ "data": { "url": ... } }`.

use std::io::Cursor;
use std::time::Duration;

use serde::Deserialize;

const DEFAULT_API_BASE: &str = "https://api.pixelvault.dev";

/// API base URL. Override with `PIXELVAULT_API_BASE` for staging/local testing.
fn api_base() -> String {
    std::env::var("PIXELVAULT_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
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

/// Upload a PNG to the keyless endpoint and return the hosted URL.
pub fn upload_png(png: Vec<u8>) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let part = reqwest::blocking::multipart::Part::bytes(png)
        .file_name("clipboard.png")
        .mime_str("image/png")
        .map_err(|e| e.to_string())?;
    let form = reqwest::blocking::multipart::Form::new().part("file", part);

    let url = format!("{}/v1/images", api_base());
    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("upload failed ({status}): {body}"));
    }

    let env: UploadEnvelope = resp.json().map_err(|e| format!("bad response: {e}"))?;
    Ok(env.data.url)
}
