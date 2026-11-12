//! Full ad blocker built on Brave's `adblock-rust` engine.
//!
//! EasyList, EasyPrivacy and RU AdList are downloaded once, parsed into a
//! serialized engine and cached on disk. Network requests are matched in the
//! WebView2 `WebResourceRequested` handler; cosmetic rules are injected into
//! the page and generic class/id rules are fed back by a small in-page helper.

use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use adblock::{Engine, FilterSet, lists::ParseOptions, request::Request};
use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;

use crate::AppEvent;

pub type FilterEngine = Engine;

const CACHE_VERSION: u32 = 1;
const CACHE_MAX_AGE: i64 = 7 * 24 * 60 * 60;
const MAX_LIST_BYTES: u64 = 24 * 1024 * 1024;

/// Subscription list used when building the engine.
const LIST_SOURCES: &[(&str, &str)] = &[
    ("EasyList", "https://easylist.to/easylist/easylist.txt"),
    (
        "EasyPrivacy",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
    (
        "RU AdList",
        "https://easylist-downloads.adblockplus.org/ruadlist.txt",
    ),
];

/// Small built-in list used when no subscription could be downloaded.
const FALLBACK_RULES: &str = r#"
||doubleclick.net^
||googlesyndication.com^
||googleadservices.com^
||adservice.google.com^
||adnxs.com^
||criteo.com^
||criteo.net^
||taboola.com^
||outbrain.com^
||scorecardresearch.com^
||quantserve.com^
||zedo.com^
||popads.net^
||popcash.net^
||propellerads.com^
||onclickads.net^
||adsterra.com^
||juicyads.com^
||exoclick.com^
||trafficjunky.net^
||mgid.com^
||revcontent.com^
||media.net^
||amazon-adsystem.com^
||rubiconproject.com^
||pubmatic.com^
||openx.net^
||smartadserver.com^
||yieldmo.com^
||adform.net^
||adsrvr.org^
||advertising.com^
||ads.yahoo.com^
||ads.yandex.ru^
||an.yandex.ru^
||directadvert.ru^
||adriver.ru^
||adfox.ru^
||luckyads.pro^
||ad-banner
##.ad-banner
##.adsbygoogle
##.advertisement
##.ad-slot
##.sponsored
"#;

#[derive(Debug, Clone, Default)]
pub struct AdblockInfo {
    pub ready: bool,
    pub loading: bool,
    pub updated_at: i64,
    pub lists: Vec<String>,
    pub message: String,
}

impl AdblockInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "ready": self.ready,
            "loading": self.loading,
            "updatedAt": self.updated_at,
            "lists": self.lists,
            "message": self.message,
        })
    }
}

pub enum AdblockEvent {
    Status(AdblockInfo),
    Ready {
        engine: Box<FilterEngine>,
        info: AdblockInfo,
    },
}

#[derive(Debug, PartialEq)]
pub enum Match {
    Allow,
    Block,
    Rewrite(String),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheMeta {
    version: u32,
    updated_at: i64,
    lists: Vec<String>,
}

fn cache_dir() -> PathBuf {
    rustle_storage::data_dir().join("adblock")
}

fn engine_path() -> PathBuf {
    cache_dir().join("engine.dat")
}

fn meta_path() -> PathBuf {
    cache_dir().join("meta.json")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn download_list(url: &str) -> Option<String> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(30))
        .call()
        .ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_LIST_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    (!text.trim().is_empty()).then_some(text)
}

fn write_cache(engine: &FilterEngine, lists: &[String]) -> i64 {
    let dir = cache_dir();
    let _ = fs::create_dir_all(&dir);
    let updated_at = now();
    let _ = fs::write(engine_path(), engine.serialize());
    let meta = CacheMeta {
        version: CACHE_VERSION,
        updated_at,
        lists: lists.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&meta) {
        let _ = fs::write(meta_path(), bytes);
    }
    updated_at
}

fn read_meta() -> Option<CacheMeta> {
    let bytes = fs::read(meta_path()).ok()?;
    let meta: CacheMeta = serde_json::from_slice(&bytes).ok()?;
    (meta.version == CACHE_VERSION).then_some(meta)
}

fn load_cached() -> Option<(FilterEngine, CacheMeta)> {
    let meta = read_meta()?;
    let bytes = fs::read(engine_path()).ok()?;
    let mut engine = Engine::default();
    engine.deserialize(&bytes).ok()?;
    Some((engine, meta))
}

fn build_engine(
    proxy: &EventLoopProxy<AppEvent>,
    download: bool,
) -> Result<(FilterEngine, AdblockInfo), String> {
    if download {
        let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Status(AdblockInfo {
            loading: true,
            message: "Скачивание фильтров…".to_owned(),
            ..AdblockInfo::default()
        })));
    }

    let mut set = FilterSet::new(false);
    let mut lists = Vec::new();
    if download {
        for (name, url) in LIST_SOURCES {
            if let Some(text) = download_list(url) {
                set.add_filter_list(text, ParseOptions::default());
                lists.push((*name).to_owned());
            }
        }
    }
    if lists.is_empty() {
        set.add_filter_list(FALLBACK_RULES.to_owned(), ParseOptions::default());
        lists.push("Встроенный список".to_owned());
    }

    let engine = Engine::new_with_filter_set(set);
    let updated_at = write_cache(&engine, &lists);
    let info = AdblockInfo {
        ready: true,
        loading: false,
        updated_at,
        lists,
        message: "Фильтры загружены".to_owned(),
    };
    Ok((engine, info))
}

/// Load the cached engine immediately (when present) and refresh it in the
/// background when it is stale or when an explicit update was requested.
pub fn spawn_loader(proxy: EventLoopProxy<AppEvent>, force: bool, allow_network: bool) {
    let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Status(AdblockInfo {
        loading: true,
        message: "Загрузка фильтров…".to_owned(),
        ..AdblockInfo::default()
    })));

    std::thread::spawn(move || {
        let cached = load_cached();
        if cached.is_none() && allow_network {
            if let Ok((engine, info)) = build_engine(&proxy, false) {
                let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Ready {
                    engine: Box::new(engine),
                    info,
                }));
            }
        }
        if let Some((engine, meta)) = cached {
            let fresh = !force && now().saturating_sub(meta.updated_at) < CACHE_MAX_AGE;
            let refresh = allow_network && !fresh;
            let info = AdblockInfo {
                ready: true,
                loading: refresh,
                updated_at: meta.updated_at,
                lists: meta.lists,
                message: if refresh {
                    "Обновление фильтров…".to_owned()
                } else {
                    "Фильтры готовы".to_owned()
                },
            };
            let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Ready {
                engine: Box::new(engine),
                info,
            }));
            if !refresh {
                return;
            }
        } else if !allow_network {
            if let Ok((engine, info)) = build_engine(&proxy, false) {
                let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Ready {
                    engine: Box::new(engine),
                    info,
                }));
            }
            return;
        }

        match build_engine(&proxy, allow_network) {
            Ok((engine, info)) => {
                let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Ready {
                    engine: Box::new(engine),
                    info,
                }));
            }
            Err(error) => {
                let info = AdblockInfo {
                    ready: false,
                    loading: false,
                    message: format!("Не удалось загрузить фильтры: {error}"),
                    ..AdblockInfo::default()
                };
                let _ = proxy.send_event(AppEvent::AdblockReady(AdblockEvent::Status(info)));
            }
        }
    });
}

pub fn build_custom(rules: &[String]) -> Option<FilterEngine> {
    let text = rules
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty() && !rule.starts_with('!'))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    let mut set = FilterSet::new(false);
    set.add_filter_list(text, ParseOptions::default());
    Some(Engine::new_with_filter_set(set))
}

pub fn check(engine: &FilterEngine, url: &str, source_url: &str, request_type: &str) -> Match {
    let Ok(request) = Request::new(url, source_url, request_type, "get") else {
        return Match::Allow;
    };
    let result = engine.check_network_request(&request);
    if result.should_block() {
        Match::Block
    } else if let Some(rewritten) = result.rewritten_url {
        if rewritten != url {
            Match::Rewrite(rewritten)
        } else {
            Match::Allow
        }
    } else {
        Match::Allow
    }
}

pub struct Cosmetic {
    pub hide_selectors: Vec<String>,
    pub generichide: bool,
    pub exceptions: HashSet<String>,
    pub injected_script: String,
}

pub fn cosmetic(engine: &FilterEngine, url: &str) -> Cosmetic {
    let resources = engine.url_cosmetic_resources(url);
    Cosmetic {
        hide_selectors: resources.hide_selectors.into_iter().collect(),
        generichide: resources.generichide,
        exceptions: resources.exceptions,
        injected_script: resources.injected_script,
    }
}

pub fn generic_selectors(
    engine: &FilterEngine,
    classes: &[String],
    ids: &[String],
    exceptions: &HashSet<String>,
) -> Vec<String> {
    engine.hidden_class_id_selectors(classes, ids, exceptions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_regex_and_cosmetic_rules_build() {
        let rules = vec![
            "/ads[0-9]+\\.js/".to_owned(),
            "example.com##.sponsored".to_owned(),
        ];
        let engine = build_custom(&rules).expect("custom engine");
        assert_eq!(
            check(
                &engine,
                "https://site.test/ads42.js",
                "https://site.test/",
                "script"
            ),
            Match::Block
        );
        let cosmetic = cosmetic(&engine, "https://example.com/page");
        assert!(
            cosmetic
                .hide_selectors
                .iter()
                .any(|selector| selector.contains("sponsored"))
        );
    }

    #[test]
    fn fallback_list_blocks_ads() {
        let mut set = FilterSet::new(false);
        set.add_filter_list(FALLBACK_RULES.to_owned(), ParseOptions::default());
        let engine = Engine::new_with_filter_set(set);
        assert!(matches!(
            check(
                &engine,
                "https://doubleclick.net/ad.js",
                "https://example.com/",
                "script"
            ),
            Match::Block
        ));
        assert!(matches!(
            check(
                &engine,
                "https://example.com/app.js",
                "https://example.com/",
                "script"
            ),
            Match::Allow
        ));
    }
}
