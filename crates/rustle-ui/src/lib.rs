//! Embedded browser chrome and internal pages.
//!
//! Assets are compiled into the binary. The app serves them through the
//! custom rustle:// protocol; nothing is loaded from the network.

use include_dir::{Dir, File, include_dir};

/// All UI assets, embedded at compile time.
pub static ASSETS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// Look up an asset by its path inside the assets directory.
pub fn asset(path: &str) -> Option<&'static File<'static>> {
    ASSETS.get_file(path)
}

/// Content type for a UI asset, based on its extension.
pub fn mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Strict CSP for every internal page. No remote scripts, no inline scripts.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    img-src 'self' data:; font-src 'self'; connect-src 'none'; frame-ancestors 'none'; \
    base-uri 'none'; form-action 'none'";
