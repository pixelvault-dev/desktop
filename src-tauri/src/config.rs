//! Shared configuration.

const DEFAULT_API_BASE: &str = "https://api.pixelvault.dev";

/// API base URL. Override with `PIXELVAULT_API_BASE` for staging/local testing.
pub fn api_base() -> String {
    std::env::var("PIXELVAULT_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}
