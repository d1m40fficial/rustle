//! Engine abstraction shared by Rustle browser backends.
//!
//! M0-M3 scope:
//! - WebView2 remains the interactive UI backend;
//! - Obscura is an opt-in browser core probe;
//! - Servo is a static renderer sidecar until interactive parity is reached.

use std::fmt;

pub const OBSCURA_VERSION: &str = "0.2.7";
pub const SERVO_VERSION: &str = "0.8.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    WebView2,
    Obscura,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Obscura,
    Servo,
}

#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub timeout_ms: u64,
}

impl RenderRequest {
    pub fn new(url: impl Into<String>, width: u32, height: u32, timeout_ms: u64) -> Self {
        Self {
            url: url.into(),
            width: width.max(64),
            height: height.max(64),
            timeout_ms: timeout_ms.max(1000),
        }
    }

    pub fn validate(&self) -> Result<(), RendererError> {
        if !(self.url.starts_with("https://") || self.url.starts_with("http://")) {
            return Err(RendererError::InvalidUrl(self.url.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub final_url: String,
    pub elapsed_ms: u128,
}

#[derive(Debug)]
pub enum RendererError {
    InvalidUrl(String),
    BinaryNotFound(&'static str),
    Spawn(std::io::Error),
    Timeout(u64),
    Failed(String),
    EmptyOutput,
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(url) => write!(f, "invalid URL: {url}"),
            Self::BinaryNotFound(name) => write!(f, "renderer binary not found: {name}"),
            Self::Spawn(error) => write!(f, "failed to spawn renderer: {error}"),
            Self::Timeout(ms) => write!(f, "renderer timed out after {ms} ms"),
            Self::Failed(message) => write!(f, "renderer failed: {message}"),
            Self::EmptyOutput => write!(f, "renderer produced no PNG"),
        }
    }
}

impl std::error::Error for RendererError {}

/// Static renderer contract used by M0-M3 benchmarks and previews.
pub trait StaticRenderer {
    fn kind(&self) -> RendererKind;
    fn render(&self, request: &RenderRequest) -> Result<RenderedPage, RendererError>;
}

pub fn png_is_valid(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_urls() {
        let request = RenderRequest::new("file:///tmp/x.html", 800, 600, 5000);
        assert!(matches!(
            request.validate(),
            Err(RendererError::InvalidUrl(_))
        ));
    }

    #[test]
    fn accepts_http_and_clamps_dimensions() {
        let request = RenderRequest::new("https://example.com", 1, 1, 100);
        assert!(request.validate().is_ok());
        assert_eq!(request.width, 64);
        assert_eq!(request.height, 64);
        assert_eq!(request.timeout_ms, 1000);
    }

    #[test]
    fn png_signature_is_checked() {
        assert!(png_is_valid(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!png_is_valid(b"not a png"));
    }
}
