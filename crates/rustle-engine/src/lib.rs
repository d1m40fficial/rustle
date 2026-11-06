//! Engine abstraction.
//!
//! M0 uses wry directly from the app crate. In M1 the BrowserEngine trait
//! and the wry backend move here, so a future CEF backend can be added
//! without touching the core.

/// Placeholder for the engine trait; implemented in M1.
pub fn planned_backend() -> &'static str {
    "wry + WebView2 (Windows), WKWebView (macOS), WebKitGTK (Linux)"
}
