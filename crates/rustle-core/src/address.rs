//! Address-bar input resolution.
//!
//! The browser accepts full URLs, bare hosts, localhost, IP addresses and plain
//! search queries. Resolution is pure so it can be unit-tested without a UI.

use serde::Serialize;
use url::Url;

/// Default search endpoint. Settings will make this configurable in M3.
pub const SEARCH_URL: &str = "https://duckduckgo.com/";

/// Human-readable error shown on the internal error view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}

/// What the user meant by the address-bar input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    Url(String),
    Search { query: String },
}

impl NavigationTarget {
    /// Convert the target into a URL that can be loaded in the content WebView.
    pub fn into_url(self) -> String {
        match self {
            Self::Url(url) => url,
            Self::Search { query } => search_url("ddg", &query),
        }
    }
}

/// Build a search URL for the selected engine. Unknown engines fall back to
/// DuckDuckGo.
pub fn search_url(engine: &str, query: &str) -> String {
    let (base, parameter) = match engine {
        "google" => ("https://www.google.com/search", "q"),
        "yandex" => ("https://yandex.ru/search/", "text"),
        "bing" => ("https://www.bing.com/search", "q"),
        _ => (SEARCH_URL, "q"),
    };
    let mut url = Url::parse(base).expect("valid search URL");
    url.query_pairs_mut().append_pair(parameter, query);
    url.to_string()
}

/// Resolve address-bar input into a URL or a search query.
///
/// Returns `None` only for empty input.
pub fn resolve_input(input: &str) -> Option<NavigationTarget> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    if let Ok(url) = Url::parse(input) {
        if is_safe_scheme(url.scheme()) {
            return Some(NavigationTarget::Url(url.to_string()));
        }
        if matches!(url.scheme(), "javascript" | "data") {
            // Never execute these from the address bar; searching is safer.
            return Some(NavigationTarget::Search {
                query: input.to_owned(),
            });
        }
    }

    if looks_like_host(input) {
        if let Ok(url) = Url::parse(&format!("https://{input}")) {
            return Some(NavigationTarget::Url(url.to_string()));
        }
    }

    Some(NavigationTarget::Search {
        query: input.to_owned(),
    })
}

/// A short, user-facing form of an internal URL.
pub fn display_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        let host = parsed.host_str().unwrap_or_default();
        let internal_origin = (parsed.scheme() == "rustle" && host == "localhost")
            || (matches!(parsed.scheme(), "http" | "https") && host == "rustle.localhost");

        if internal_origin {
            let path = parsed.path().trim_start_matches('/');
            if path.starts_with("newtab") {
                return "rustle://newtab".to_owned();
            }
            for page in ["history", "bookmarks", "settings", "about", "downloads"] {
                if path.starts_with(page) {
                    return format!("rustle://{page}");
                }
            }
        }
    }
    url.to_owned()
}

fn is_safe_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "file" | "rustle" | "about")
}

fn looks_like_host(input: &str) -> bool {
    if input.chars().any(char::is_whitespace) {
        return false;
    }
    if input.starts_with("localhost") || input.starts_with("127.") || input.starts_with('[') {
        return true;
    }

    let host = input.split('/').next().unwrap_or(input);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::{NavigationTarget, display_url, resolve_input, search_url};

    #[test]
    fn keeps_absolute_https_url() {
        assert_eq!(
            resolve_input("https://example.com/path"),
            Some(NavigationTarget::Url("https://example.com/path".to_owned()))
        );
    }

    #[test]
    fn adds_https_to_bare_host() {
        assert_eq!(
            resolve_input("example.com"),
            Some(NavigationTarget::Url("https://example.com/".to_owned()))
        );
    }

    #[test]
    fn treats_localhost_with_port_as_url() {
        assert_eq!(
            resolve_input("localhost:8080"),
            Some(NavigationTarget::Url("https://localhost:8080/".to_owned()))
        );
    }

    #[test]
    fn treats_plain_words_as_search() {
        assert_eq!(
            resolve_input("hello world"),
            Some(NavigationTarget::Search {
                query: "hello world".to_owned()
            })
        );
    }

    #[test]
    fn treats_javascript_as_search() {
        assert_eq!(
            resolve_input("javascript:alert(1)"),
            Some(NavigationTarget::Search {
                query: "javascript:alert(1)".to_owned()
            })
        );
    }

    #[test]
    fn empty_input_has_no_target() {
        assert_eq!(resolve_input("   "), None);
    }

    #[test]
    fn search_url_uses_selected_engine() {
        assert_eq!(
            search_url("yandex", "rustle browser"),
            "https://yandex.ru/search/?text=rustle+browser"
        );
    }

    #[test]
    fn search_url_uses_duckduckgo() {
        let target = resolve_input("rustle browser").expect("target");
        assert_eq!(
            target.into_url(),
            "https://duckduckgo.com/?q=rustle+browser"
        );
    }

    #[test]
    fn displays_internal_newtab_url() {
        assert_eq!(
            display_url("rustle://localhost/newtab/index.html"),
            "rustle://newtab"
        );
        assert_eq!(
            display_url("http://rustle.localhost/newtab/index.html"),
            "rustle://newtab"
        );
    }
}
