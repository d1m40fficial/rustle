#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Rustle desktop browser.

mod adblock;
mod single_instance;
mod storage_worker;

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    rc::Rc,
    sync::OnceLock,
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use rustle_core::{ErrorInfo, TabId, VERSION, display_url, resolve_input, search_url};
use rustle_storage::{
    BookmarkEntry, DownloadRecord, SESSION_VERSION, Session, SessionGroup, SessionTab, Settings,
};
use rustle_ui::{CSP, asset, mime_type};
use serde_json::{Value, json};
use storage_worker::{StorageCommand, StorageQuery};
use tao::{
    dpi::LogicalSize as TaoLogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Fullscreen, Icon, Window, WindowBuilder},
};
use url::Url;
use wry::{
    MemoryUsageLevel, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewId,
    dpi::{LogicalPosition, LogicalSize},
    http::{Request, Response},
};

#[cfg(target_os = "windows")]
use tao::platform::windows::WindowExtWindows;
#[cfg(target_os = "windows")]
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};

#[cfg(target_os = "windows")]
use webview2_com::{
    BytesReceivedChangedEventHandler, ClearBrowsingDataCompletedHandler,
    DownloadStartingEventHandler, FaviconChangedEventHandler, GetFaviconCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
        ICoreWebView2_4, ICoreWebView2_7, ICoreWebView2_8, ICoreWebView2_13, ICoreWebView2_15,
        ICoreWebView2DownloadOperation, ICoreWebView2PrintToPdfCompletedHandler,
        ICoreWebView2Profile2,
    },
    StateChangedEventHandler, WebResourceRequestedEventHandler, ZoomFactorChangedEventHandler,
};
#[cfg(target_os = "windows")]
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE,
        COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS,
        COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    },
    PermissionRequestedEventHandler, ProcessFailedEventHandler,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::IStream;
use windows_core::{Interface, PCWSTR};

const CHROME_COLLAPSED: u32 = 78;
const CHROME_VERTICAL: u32 = 44;
const SIDEBAR_WIDTH: u32 = 282;
const SIDEBAR_COLLAPSED: u32 = 56;
const CHROME_URL: &str = "rustle://localhost/chrome/index.html";
const NEWTAB_URL: &str = "rustle://localhost/newtab/index.html";

const RESIZE_JS: &str = r#"(function(){
  if (window.__rustleResizeInstalled) return;
  window.__rustleResizeInstalled = true;
  var edgeSize = 10;
  var edges = [
    {edge: 11, css: "top:0;right:0;width:10px;height:100%;cursor:ew-resize"},
    {edge: 10, css: "top:0;left:0;width:10px;height:100%;cursor:ew-resize"},
    {edge: 15, css: "left:0;bottom:0;width:100%;height:10px;cursor:ns-resize"},
    {edge: 17, css: "right:0;bottom:0;width:20px;height:20px;cursor:nwse-resize"},
    {edge: 16, css: "left:0;bottom:0;width:20px;height:20px;cursor:nesw-resize"}
  ];
  edges.forEach(function(item){
    var node = document.createElement("div");
    node.style.cssText = "position:fixed;z-index:2147483647;touch-action:none;" + item.css;
    node.addEventListener("pointerdown", function(event){
      event.preventDefault();
      node.setPointerCapture(event.pointerId);
      if (window.ipc) window.ipc.postMessage(JSON.stringify({type:"window.resizeStart", edge:item.edge, screenX:event.screenX, screenY:event.screenY}));
    });
    node.addEventListener("pointermove", function(event){
      if (!node.hasPointerCapture(event.pointerId)) return;
      if (window.ipc) window.ipc.postMessage(JSON.stringify({type:"window.resizeMove", screenX:event.screenX, screenY:event.screenY}));
    });
    node.addEventListener("pointerup", function(event){
      if (node.hasPointerCapture(event.pointerId)) node.releasePointerCapture(event.pointerId);
      if (window.ipc) window.ipc.postMessage(JSON.stringify({type:"window.resizeEnd"}));
    });
    document.documentElement.appendChild(node);
  });
})();"#;

const SIDEBAR_URL: &str = "rustle://localhost/sidebar/index.html";
const MAX_IPC_BYTES: usize = 64 * 1024;
const MAX_CLOSED_TABS: usize = 25;

/// WebView2 uses these switches to label its process tree in Task Manager.
/// Without them Windows shows the generic "WebView2 Manager" name.
fn app_icon() -> Icon {
    let image = include_bytes!("../assets/rustle-icon.rgba");
    Icon::from_rgba(image.to_vec(), 256, 256).expect("valid application icon")
}

fn webview_browser_args() -> String {
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --autoplay-policy=no-user-gesture-required \
         --webview-exe-name=Rustle --webview-exe-version={VERSION} --app-user-model-id=Rustle"
    )
}

static START: OnceLock<Instant> = OnceLock::new();

fn mark(label: &str) {
    let start = START.get_or_init(Instant::now);
    println!("[startup] {:>5}ms {label}", start.elapsed().as_millis());
}

enum AppEvent {
    OpenUrl(String),
    StorageResponse { request_id: u64, payload: Value },
    DownloadsChanged,
    DownloadsRestored(Vec<DownloadRecord>),
    BookmarksRestored(Vec<BookmarkEntry>),
    LoadTimeout(TabId),
    ContentIpc(TabId, String),
    RefreshFavicon(TabId),
    FetchFavicons,
    FaviconFetched(TabId, String, Vec<u8>),
    TileFavicons { request_id: u64, items: Value },
    AdblockReady(adblock::AdblockEvent),
    Autosave,
    Maintenance,
    UpdateStatus(String),
    RefreshResize,
    Vertical(bool),
    CreateGroup { name: String, tab: Option<TabId> },
    ToggleGroup(u64),
    RemoveGroup(u64),
    NewTabInGroup(u64),
    MoveTabGroup { tab: TabId, group: Option<u64> },
    MoveTab { tab: TabId, target: TabId },
    LayoutTick,
    Focus,
    NewTab,
    Activate(TabId),
    Close(TabId),
    Reopen,
    Navigate(String),
    Title(TabId, String),
    Exit,
}

struct Tab {
    id: TabId,
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    error: Option<ErrorInfo>,
    webview: Option<WebView>,
    nav_id: u64,
    return_url: Option<String>,
    return_title: Option<String>,
    favicon: Option<String>,
    zoom: f64,
    group: Option<u64>,
    pinned: bool,
    muted: bool,
    container: Option<String>,
    blocked: u64,
    cosmetic_generic: bool,
    cosmetic_exceptions: HashSet<String>,
    cosmetic_injected: HashSet<String>,
    last_active: Instant,
}

#[derive(Debug, Clone)]
struct TabGroup {
    id: u64,
    name: String,
    collapsed: bool,
    hue: u16,
}

impl Tab {
    fn new(id: TabId, url: String, title: String) -> Self {
        Self {
            id,
            url,
            title,
            loading: false,
            can_back: false,
            can_forward: false,
            error: None,
            webview: None,
            nav_id: 0,
            return_url: None,
            return_title: None,
            favicon: None,
            zoom: 1.0,
            group: None,
            pinned: false,
            muted: false,
            container: None,
            blocked: 0,
            cosmetic_generic: true,
            cosmetic_exceptions: HashSet::new(),
            cosmetic_injected: HashSet::new(),
            last_active: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
struct ClosedTab {
    url: String,
    title: String,
}

struct DownloadEntry {
    id: u64,
    url: String,
    path: String,
    received: i64,
    total: i64,
    state: String,
    paused: bool,
    #[cfg(target_os = "windows")]
    operation: Option<ICoreWebView2DownloadOperation>,
}

struct AppState {
    window: Rc<Window>,
    context: Option<WebContext>,
    chrome: Option<WebView>,
    tabs: Vec<Tab>,
    active: TabId,
    next_tab_id: u64,
    closed: Vec<ClosedTab>,
    width: u32,
    height: u32,
    ready: bool,
    proxy: EventLoopProxy<AppEvent>,
    last_save: Instant,
    stress_cycles: Option<u32>,
    settings: Settings,
    storage: Option<Sender<StorageCommand>>,
    dropdown_open: bool,
    dropdown_height: u32,
    bookmarks: Vec<BookmarkEntry>,
    find_open: bool,
    permissions: HashMap<String, bool>,
    downloads: Vec<DownloadEntry>,
    next_download_id: u64,
    private: bool,
    sidebar: Option<WebView>,
    vertical: bool,
    sidebar_collapsed: bool,
    layout_anim: Option<LayoutAnimation>,
    groups: Vec<TabGroup>,
    next_group_id: u64,
    adblock_engine: Option<adblock::FilterEngine>,
    adblock_custom_engine: Option<adblock::FilterEngine>,
    adblock_info: adblock::AdblockInfo,
    prompt_open: bool,
    switcher_open: bool,
    recovered: bool,
    saved_sessions: Vec<Value>,
    webview2_version: String,
    update_status: String,
    resize_session: Option<ResizeSession>,
    settings_dirty: bool,
}

struct LayoutAnimation {
    started: Instant,
    from_top: u32,
    to_top: u32,
    from_side: u32,
    to_side: u32,
}

impl AppState {
    fn new(
        window: Rc<Window>,
        context: WebContext,
        width: u32,
        height: u32,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self {
            window,
            context: Some(context),
            chrome: None,
            tabs: Vec::new(),
            active: TabId(0),
            next_tab_id: 1,
            closed: Vec::new(),
            width,
            height,
            ready: false,
            proxy,
            last_save: Instant::now(),
            stress_cycles: None,
            settings: Settings::default(),
            storage: None,
            dropdown_open: false,
            dropdown_height: 360,
            bookmarks: Vec::new(),
            find_open: false,
            permissions: HashMap::new(),
            downloads: Vec::new(),
            next_download_id: 1,
            private: false,
            sidebar: None,
            vertical: false,
            sidebar_collapsed: false,
            layout_anim: None,
            groups: Vec::new(),
            next_group_id: 1,
            adblock_engine: None,
            adblock_custom_engine: None,
            adblock_info: adblock::AdblockInfo::default(),
            prompt_open: false,
            switcher_open: false,
            recovered: false,
            saved_sessions: Vec::new(),
            webview2_version: String::new(),
            update_status: String::new(),
            resize_session: None,
            settings_dirty: false,
        }
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tab(self.active)
    }
}

fn data_directory() -> PathBuf {
    let directory = rustle_storage::data_dir().join("WebView2");
    let _ = std::fs::create_dir_all(&directory);
    directory
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect {
        position: LogicalPosition::new(x as f64, y as f64).into(),
        size: LogicalSize::new(width, height).into(),
    }
}

fn text_response(status: u16, body: &'static str) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-content-type-options", "nosniff")
        .body(Cow::Borrowed(body.as_bytes()))
        .expect("valid text response")
}

fn protocol_handler(_id: WebViewId, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    let path = if path.is_empty() {
        "newtab/index.html"
    } else {
        path
    };

    if path.contains("..") || path.contains('\\') {
        return text_response(400, "bad request");
    }

    if let Some(name) = path.strip_prefix("favicon/") {
        if name.ends_with(".png") && name.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '.') {
            if let Ok(bytes) = fs::read(favicon_cache_path(name)) {
                return Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .header("x-content-type-options", "nosniff")
                    .body(Cow::Owned(bytes))
                    .expect("valid favicon response");
            }
        }
    }

    match asset(path) {
        Some(file) => Response::builder()
            .status(200)
            .header("content-type", mime_type(path))
            .header("content-security-policy", CSP)
            .header("x-content-type-options", "nosniff")
            .body(Cow::Borrowed(file.contents()))
            .expect("valid asset response"),
        None => text_response(404, "not found"),
    }
}

fn canonicalize_internal(url: String) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed == "rustle://newtab" || url.contains("rustle.localhost/newtab") {
        NEWTAB_URL.to_owned()
    } else if let Some(page) = internal_page(&url) {
        format!("rustle://{page}")
    } else {
        url
    }
}

fn internal_page(url: &str) -> Option<&'static str> {
    let internal = url.starts_with("rustle://") || url.contains("rustle.localhost/");
    if !internal {
        return None;
    }
    if url.contains("history") {
        Some("history")
    } else if url.contains("bookmarks") {
        Some("bookmarks")
    } else if url.contains("settings") {
        Some("settings")
    } else if url.contains("about") {
        Some("about")
    } else if url.contains("downloads") {
        Some("downloads")
    } else {
        None
    }
}

fn internal_title(page: &str) -> &'static str {
    match page {
        "history" => "История",
        "bookmarks" => "Закладки",
        "settings" => "Настройки",
        "about" => "О Rustle",
        "downloads" => "Загрузки",
        _ => "Rustle",
    }
}

fn is_overlay_url(url: &str) -> bool {
    is_newtab_url(url) || internal_page(url).is_some()
}

fn session_url(url: &str) -> String {
    if url.starts_with("rustle://newtab") || url.contains("rustle.localhost/newtab") {
        NEWTAB_URL.to_owned()
    } else if let Some(page) = internal_page(url) {
        format!("rustle://{page}")
    } else {
        url.to_owned()
    }
}

fn is_newtab_url(url: &str) -> bool {
    url.starts_with("rustle://newtab")
        || url.contains("rustle://localhost/newtab")
        || url.contains("rustle.localhost/newtab")
}

fn newtab_overlay(state: &AppState) -> bool {
    state
        .active_tab()
        .is_some_and(|tab| tab.webview.is_none() && is_newtab_url(&tab.url))
}

fn fallback_title(url: &str) -> String {
    if is_newtab_url(url) {
        return "Новая вкладка".to_owned();
    }
    if let Some(page) = internal_page(url) {
        return internal_title(page).to_owned();
    }
    if url.starts_with("rustle://") || url.contains("rustle.localhost") {
        return "Rustle".to_owned();
    }
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Новая вкладка".to_owned())
}

fn effective_tab_title(tab: &Tab) -> String {
    if tab.error.is_some() {
        return "Не удалось открыть страницу".to_owned();
    }
    if !tab.title.is_empty() {
        return tab.title.clone();
    }
    fallback_title(&tab.url)
}

fn push_tab(state: &mut AppState, url: String, title: String) -> TabId {
    let id = TabId(state.next_tab_id);
    state.next_tab_id += 1;
    let mut tab = Tab::new(id, url, title);
    tab.container = state.settings.default_container.clone();
    state.tabs.push(tab);
    id
}

fn save_session_state(state: &Rc<RefCell<AppState>>, clean_exit: bool) {
    if state.borrow().private {
        return;
    }
    let session = {
        let state = state.borrow();
        let active = state.tab_index(state.active).unwrap_or(0);
        Session {
            version: SESSION_VERSION,
            clean_exit,
            active,
            tabs: state
                .tabs
                .iter()
                .map(|tab| SessionTab {
                    url: session_url(&tab.url),
                    title: tab.title.clone(),
                    group: tab.group,
                    pinned: tab.pinned,
                    muted: tab.muted,
                    container: tab.container.clone(),
                })
                .collect(),
            closed: state
                .closed
                .iter()
                .map(|tab| SessionTab {
                    url: session_url(&tab.url),
                    title: tab.title.clone(),
                    group: None,
                    pinned: false,
                    muted: false,
                    container: None,
                })
                .collect(),
            groups: state
                .groups
                .iter()
                .map(|group| SessionGroup {
                    id: group.id,
                    name: group.name.clone(),
                    collapsed: group.collapsed,
                    hue: group.hue,
                })
                .collect(),
        }
    };

    if let Err(error) = rustle_storage::save_session(&session) {
        eprintln!("[session] save failed: {error}");
    }
    state.borrow_mut().last_save = Instant::now();
}

fn save_session_due(state: &Rc<RefCell<AppState>>) {
    let due = state.borrow().last_save.elapsed() >= Duration::from_millis(800);
    if due {
        save_session_state(state, false);
    }
}

fn restore_session(state: &Rc<RefCell<AppState>>, session: Option<Session>) {
    let mut state = state.borrow_mut();
    let session = session.unwrap_or_default();

    let closed = session.closed;
    state.groups = session
        .groups
        .into_iter()
        .filter(|group| !group.name.trim().is_empty())
        .map(|group| TabGroup {
            id: group.id,
            name: group.name,
            collapsed: group.collapsed,
            hue: group.hue,
        })
        .collect();
    state.next_group_id = state.groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
    if session.tabs.is_empty() {
        let id = push_tab(
            &mut state,
            NEWTAB_URL.to_owned(),
            "Новая вкладка".to_owned(),
        );
        state.active = id;
    } else {
        for tab in session.tabs {
            let group = tab
                .group
                .filter(|id| state.groups.iter().any(|group| group.id == *id));
            let url = canonicalize_internal(if tab.url.is_empty() {
                NEWTAB_URL.to_owned()
            } else {
                tab.url
            });
            let favicon = cached_favicon(&url);
            let id = push_tab(&mut state, url, tab.title);
            if let Some(entry) = state.tab_mut(id) {
                entry.favicon = favicon;
                entry.pinned = tab.pinned;
                entry.muted = tab.muted;
                entry.container = tab.container;
                entry.group = if tab.pinned { None } else { group };
            }
        }
        let active_index = session.active.min(state.tabs.len().saturating_sub(1));
        let active_id = state.tabs[active_index].id;
        state.tabs.sort_by_key(|tab| !tab.pinned);
        state.active = active_id;
        let active_collapsed = state
            .tab(state.active)
            .and_then(|tab| tab.group)
            .and_then(|id| state.groups.iter().find(|group| group.id == id))
            .is_some_and(|group| group.collapsed);
        if active_collapsed {
            if let Some(visible) = state.tabs.iter().find(|tab| {
                tab.group
                    .and_then(|id| state.groups.iter().find(|group| group.id == id))
                    .is_none_or(|group| !group.collapsed)
            }) {
                state.active = visible.id;
            } else if let Some(group) = state.groups.iter_mut().find(|group| group.collapsed) {
                group.collapsed = false;
            }
        }
    }
    state.closed = closed
        .into_iter()
        .take(MAX_CLOSED_TABS)
        .map(|tab| ClosedTab {
            url: canonicalize_internal(tab.url),
            title: tab.title,
        })
        .collect();
}

fn push_ui_state(state: &Rc<RefCell<AppState>>) {
    let (payload, window_title) = {
        let state = state.borrow();
        if !state.ready {
            return;
        }

        let active = state.active_tab();
        let tabs: Vec<Value> = state
            .tabs
            .iter()
            .map(|tab| {
                json!({
                    "id": tab.id.0,
                    "title": effective_tab_title(tab),
                    "url": display_url(&tab.url),
                    "favicon": tab.favicon,
                    "faviconData": tab.favicon.as_deref().and_then(favicon_data),
                    "loading": tab.loading,
                    "active": tab.id == state.active,
                    "groupId": tab.group,
                    "pinned": tab.pinned,
                    "muted": tab.muted,
                    "container": tab.container,
                    "blocked": tab.blocked,
                    "error": tab.error.clone(),
                })
            })
            .collect();

        let (url, title, loading, can_back, can_forward, error) = active
            .map(|tab| {
                (
                    tab.url.clone(),
                    effective_tab_title(tab),
                    tab.loading,
                    tab.can_back,
                    tab.can_forward,
                    tab.error.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    String::new(),
                    "Rustle".to_owned(),
                    false,
                    false,
                    false,
                    None,
                )
            });

        let (mode, internal_page_name, overlay_back) = if error.is_some() {
            ("error", "", false)
        } else if active.is_some_and(|tab| tab.webview.is_none() && is_newtab_url(&tab.url)) {
            (
                "newtab",
                "",
                active.is_some_and(|tab| tab.return_url.is_some()),
            )
        } else if let Some(page) = active.and_then(|tab| internal_page(&tab.url)) {
            (
                "internal",
                page,
                active.is_some_and(|tab| tab.return_url.is_some()),
            )
        } else {
            ("web", "", false)
        };

        let payload = json!({
            "tabs": tabs,
            "activeId": state.active.0,
            "url": url,
            "displayUrl": display_url(&url),
            "title": title,
            "loading": loading,
            "canGoBack": if mode == "web" || mode == "error" { can_back } else { overlay_back },
            "canGoForward": if mode == "web" || mode == "error" { can_forward } else { false },
            "error": error,
            "mode": mode,
            "internalPage": internal_page_name,
            "theme": if state.settings.theme == "light" { "light" } else { "dark" },
            "language": state.settings.language,
            "private": state.private,
            "zoom": active.map(|tab| tab.zoom).unwrap_or(1.0),
            "vertical": state.vertical,
            "sidebarCollapsed": state.sidebar_collapsed,
            "sidebarWidth": state.settings.sidebar_width.clamp(72, SIDEBAR_WIDTH),
            "switcherOpen": state.switcher_open,
            "recovered": state.recovered,
            "sessions": state.saved_sessions,
            "webview2Version": state.webview2_version,
            "updateStatus": state.update_status,
            "groups": state.groups.iter().map(|group| json!({"id": group.id, "name": group.name, "collapsed": group.collapsed})).collect::<Vec<_>>(),
            "secure": url.starts_with("https://"),
            "bookmarks": state.bookmarks,
            "settings": {
                "searchEngine": state.settings.search_engine,
                "theme": state.settings.theme,
                "language": state.settings.language,
                "particlesEnabled": state.settings.particles_enabled,
                "particleIntensity": state.settings.particle_intensity,
                "containers": state.settings.containers,
                "defaultContainer": state.settings.default_container,
                "startup": state.settings.startup,
                "httpsFirst": state.settings.https_first,
                "clearOnExit": state.settings.clear_on_exit,
                "adblock": state.settings.adblock,
                "adblockExceptions": state.settings.adblock_exceptions,
                "autoSuspendMinutes": state.settings.auto_suspend_minutes,
            },
            "adblockStatus": state.adblock_info.to_json(),
            "adblockExceptions": state.settings.adblock_exceptions,
            "downloads": downloads_payload(&state),
        });

        let window_title = if title == "Новая вкладка" || title == "Rustle" {
            if state.private {
                "Rustle — приватное окно".to_owned()
            } else {
                "Rustle".to_owned()
            }
        } else if state.private {
            format!("{title} — Rustle (приватно)")
        } else {
            format!("{title} — Rustle")
        };

        (payload, window_title)
    };

    state.borrow().window.set_title(&window_title);
    let script = format!(
        "window.rustle && window.rustle.dispatch({{\"type\":\"state\",\"payload\":{payload}}});"
    );
    let state_ref = state.borrow();
    if let Some(chrome) = &state_ref.chrome {
        if let Err(error) = chrome.evaluate_script(&script) {
            eprintln!("[ui] state push failed: {error}");
        }
    }
    if let Some(sidebar) = &state_ref.sidebar {
        let _ = sidebar.evaluate_script(&script);
    }
}

fn send_app_info(state: &Rc<RefCell<AppState>>) {
    let state = state.borrow();
    let Some(chrome) = &state.chrome else {
        return;
    };

    let info = json!({
        "type": "app.info",
        "version": VERSION,
        "platform": std::env::consts::OS,
    });
    let script = format!("window.rustle && window.rustle.dispatch({info});");
    if let Err(error) = chrome.evaluate_script(&script) {
        eprintln!("[ipc] app.info failed: {error}");
    }
}

#[cfg(target_os = "windows")]
fn history_flags(content: &WebView) -> (bool, bool) {
    use windows_core::BOOL;

    let webview = content.webview();
    let mut back = BOOL(0);
    let mut forward = BOOL(0);
    unsafe {
        let _ = webview.CanGoBack(&mut back);
        let _ = webview.CanGoForward(&mut forward);
    }
    (back.as_bool(), forward.as_bool())
}

#[cfg(not(target_os = "windows"))]
fn history_flags(_content: &WebView) -> (bool, bool) {
    (false, false)
}

fn sync_tab_from_webview(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    let snapshot = {
        let state = state.borrow();
        let Some(tab) = state.tab(tab_id) else {
            return;
        };
        let Some(content) = &tab.webview else {
            return;
        };
        let url = content.url().unwrap_or_else(|_| tab.url.clone());
        let (back, forward) = history_flags(content);
        (url, back, forward)
    };

    let mut state = state.borrow_mut();
    if let Some(tab) = state.tab_mut(tab_id) {
        if tab.error.is_none() {
            tab.url = snapshot.0;
        }
        tab.can_back = snapshot.1;
        tab.can_forward = snapshot.2;
    }
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn ReleaseCapture() -> i32;
    fn SendMessageW(hwnd: isize, msg: u32, wparam: isize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: isize, command: i32) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn SetWindowPos(
        hwnd: isize,
        insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
}

fn raise_chrome(state: &Rc<RefCell<AppState>>) {
    #[cfg(target_os = "windows")]
    {
        const HWND_TOP: isize = 0;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOACTIVATE: u32 = 0x0010;

        let hwnd = {
            let state = state.borrow();
            state.chrome.as_ref().map(|chrome| chrome.hwnd().0 as isize)
        };
        if let Some(hwnd) = hwnd {
            unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

fn raise_sidebar(state: &Rc<RefCell<AppState>>) {
    #[cfg(target_os = "windows")]
    {
        const HWND_TOP: isize = 0;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOACTIVATE: u32 = 0x0010;
        let hwnd = {
            let state = state.borrow();
            state
                .sidebar
                .as_ref()
                .map(|sidebar| sidebar.hwnd().0 as isize)
        };
        if let Some(hwnd) = hwnd {
            unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod native_menu {
    use super::{AppState, TabId};
    use std::cell::RefCell;
    use std::rc::Rc;
    use tao::platform::windows::WindowExtWindows;
    use wry::WebViewExtWindows;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ClientToScreen(hwnd: isize, point: *mut Point) -> i32;
        fn CreatePopupMenu() -> isize;
        fn DestroyMenu(menu: isize) -> i32;
        fn AppendMenuW(menu: isize, flags: u32, id: usize, text: *const u16) -> i32;
        fn TrackPopupMenu(
            menu: isize,
            flags: u32,
            x: i32,
            y: i32,
            reserved: i32,
            hwnd: isize,
            rect: *const core::ffi::c_void,
        ) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }

    const MF_STRING: u32 = 0x0000;
    const MF_SEPARATOR: u32 = 0x0800;
    const MF_CHECKED: u32 = 0x0008;
    const TPM_RETURNCMD: u32 = 0x0100;
    const TPM_NONOTIFY: u32 = 0x0080;
    const TPM_RIGHTBUTTON: u32 = 0x0002;

    #[derive(Clone, Copy)]
    pub enum Origin {
        Chrome,
        Sidebar,
    }

    pub enum Kind {
        App,
        Tab(TabId),
        Group(u64),
    }

    struct Entry {
        action: String,
        label: String,
        checked: bool,
        separator: bool,
    }

    fn push(items: &mut Vec<Entry>, action: &str, label: &str) {
        items.push(Entry {
            action: action.to_owned(),
            label: label.to_owned(),
            checked: false,
            separator: false,
        });
    }

    fn push_checked(items: &mut Vec<Entry>, action: &str, label: &str, checked: bool) {
        items.push(Entry {
            action: action.to_owned(),
            label: label.to_owned(),
            checked,
            separator: false,
        });
    }

    fn separator(items: &mut Vec<Entry>) {
        items.push(Entry {
            action: String::new(),
            label: String::new(),
            checked: false,
            separator: true,
        });
    }

    fn translate_menu_label(label: &str) -> String {
        let exact = match label {
            "Новая вкладка" => "New tab",
            "Вернуть закрытую вкладку" => "Reopen closed tab",
            "Библиотека сессий" => "Session library",
            "Сохранить сессию…" => "Save session…",
            "Открыть сессию…" => "Open session…",
            "Сгруппировать по доменам" => "Group by domain",
            "История" => "History",
            "Закладки" => "Bookmarks",
            "Загрузки" => "Downloads",
            "Настройки" => "Settings",
            "Добавить в закладки" => "Add bookmark",
            "Отключить Adblock на этом сайте" => "Disable Adblock on this site",
            "Сохранить страницу в PDF…" => "Save page as PDF…",
            "Скриншот страницы…" => "Screenshot page…",
            "Вертикальные вкладки" => "Vertical tabs",
            "Закрепить вкладку" => "Pin tab",
            "Открепить вкладку" => "Unpin tab",
            "Выключить звук вкладки" => "Mute tab",
            "Включить звук вкладки" => "Unmute tab",
            "Новая группа…" => "New group…",
            "Убрать из группы" => "Remove from group",
            "Закрыть вкладку" => "Close tab",
            "Развернуть группу" => "Expand group",
            "Свернуть группу" => "Collapse group",
            "Новая вкладка в группе" => "New tab in group",
            "Сменить цвет группы" => "Change group color",
            "Переименовать группу…" => "Rename group…",
            "Удалить группу" => "Delete group",
            "Без контейнера" => "No container",
            _ => "",
        };
        if !exact.is_empty() {
            return exact.to_owned();
        }
        if let Some(name) = label.strip_prefix("В группу «") {
            return format!("Move to group «{}", name);
        }
        label.to_owned()
    }

    fn app_items(state: &AppState, items: &mut Vec<Entry>) {
        push(items, "app.newtab", "Новая вкладка");
        push(items, "app.reopen", "Вернуть закрытую вкладку");
        separator(items);
        push(items, "app.sessions", "Библиотека сессий");
        push(items, "app.unloadInactive", "Выгрузить неактивные вкладки");
        push(items, "app.session.save", "Сохранить сессию…");
        push(items, "app.session.open", "Открыть сессию…");
        push(items, "app.autogroup", "Сгруппировать по доменам");
        push(items, "app.history", "История");
        push(items, "app.bookmarks", "Закладки");
        push(items, "app.downloads", "Загрузки");
        push(items, "app.settings", "Настройки");
        if state
            .active_tab()
            .is_some_and(|tab| tab.url.starts_with("http"))
        {
            push(items, "app.bookmark", "Добавить в закладки");
            push(items, "app.whitelist", "Отключить Adblock на этом сайте");
            push(items, "app.pdf", "Сохранить страницу в PDF…");
            push(items, "app.screenshot", "Скриншот страницы…");
        }
        separator(items);
        push_checked(
            items,
            "app.vertical",
            "Вертикальные вкладки",
            state.vertical,
        );
    }

    pub fn show(state: &Rc<RefCell<AppState>>, kind: Kind, origin: Origin, x: i32, y: i32) {
        let (hwnd, owner, scale, mut entries, language) = {
            let state = state.borrow();
            let scale = state.window.scale_factor();
            let owner = state.window.hwnd();
            let hwnd = match origin {
                Origin::Chrome => state.chrome.as_ref().map(|view| view.hwnd().0 as isize),
                Origin::Sidebar => state.sidebar.as_ref().map(|view| view.hwnd().0 as isize),
            };
            let Some(hwnd) = hwnd else {
                return;
            };
            let mut entries = Vec::new();
            match kind {
                Kind::App => app_items(&state, &mut entries),
                Kind::Tab(id) => {
                    if let Some(tab) = state.tab(id) {
                        if tab.pinned {
                            push(
                                &mut entries,
                                &format!("tab.unpin:{}", id.0),
                                "Открепить вкладку",
                            );
                        } else {
                            push(
                                &mut entries,
                                &format!("tab.pin:{}", id.0),
                                "Закрепить вкладку",
                            );
                        }
                        push_checked(
                            &mut entries,
                            &format!("tab.container:{}:none", id.0),
                            "Без контейнера",
                            tab.container.is_none(),
                        );
                        for container in &state.settings.containers {
                            push_checked(
                                &mut entries,
                                &format!("tab.container:{}:{}", id.0, container),
                                container,
                                tab.container.as_deref() == Some(container.as_str()),
                            );
                        }
                        separator(&mut entries);
                        push_checked(
                            &mut entries,
                            &format!("tab.mute:{}", id.0),
                            if tab.muted {
                                "Включить звук вкладки"
                            } else {
                                "Выключить звук вкладки"
                            },
                            tab.muted,
                        );
                        if !tab.pinned {
                            separator(&mut entries);
                            push(
                                &mut entries,
                                &format!("tab.newgroup:{}", id.0),
                                "Новая группа…",
                            );
                            for group in &state.groups {
                                if tab.group == Some(group.id) {
                                    continue;
                                }
                                push(
                                    &mut entries,
                                    &format!("tab.group:{}:{}", id.0, group.id),
                                    &format!("В группу «{}»", group.name),
                                );
                            }
                            if tab.group.is_some() {
                                push(
                                    &mut entries,
                                    &format!("tab.ungroup:{}", id.0),
                                    "Убрать из группы",
                                );
                            }
                        }
                        separator(&mut entries);
                        push(
                            &mut entries,
                            &format!("tab.close:{}", id.0),
                            "Закрыть вкладку",
                        );
                    }
                }
                Kind::Group(group_id) => {
                    if let Some(group) = state.groups.iter().find(|group| group.id == group_id) {
                        push_checked(
                            &mut entries,
                            &format!("group.toggle:{group_id}"),
                            if group.collapsed {
                                "Развернуть группу"
                            } else {
                                "Свернуть группу"
                            },
                            group.collapsed,
                        );
                        push(
                            &mut entries,
                            &format!("group.newtab:{group_id}"),
                            "Новая вкладка в группе",
                        );
                        push(
                            &mut entries,
                            &format!("group.color:{group_id}"),
                            "Сменить цвет группы",
                        );
                        push(
                            &mut entries,
                            &format!("group.rename:{group_id}"),
                            "Переименовать группу…",
                        );
                        push(
                            &mut entries,
                            &format!("group.remove:{group_id}"),
                            "Удалить группу",
                        );
                    }
                }
            }
            (hwnd, owner, scale, entries, state.settings.language.clone())
        };
        if language == "en" {
            for entry in &mut entries {
                entry.label = translate_menu_label(&entry.label);
            }
        }

        if entries.is_empty() {
            return;
        }

        let mut point = Point {
            x: (x as f64 * scale).round() as i32,
            y: (y as f64 * scale).round() as i32,
        };
        unsafe {
            if ClientToScreen(hwnd, &mut point) == 0 {
                return;
            }
        }

        let menu = unsafe { CreatePopupMenu() };
        if menu == 0 {
            return;
        }
        let mut actions = Vec::with_capacity(entries.len());
        for (index, entry) in entries.into_iter().enumerate() {
            if entry.separator {
                unsafe {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                }
                actions.push(String::new());
                continue;
            }
            let mut wide: Vec<u16> = entry.label.replace('&', "&&").encode_utf16().collect();
            wide.push(0);
            let flags = MF_STRING | if entry.checked { MF_CHECKED } else { 0 };
            unsafe {
                AppendMenuW(menu, flags, index + 1, wide.as_ptr());
            }
            actions.push(entry.action);
        }

        unsafe {
            SetForegroundWindow(owner);
        }
        let command = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                owner,
                std::ptr::null(),
            )
        };
        unsafe {
            DestroyMenu(menu);
        }
        if command <= 0 {
            return;
        }
        let Some(action) = actions.get(command as usize - 1).cloned() else {
            return;
        };
        if action.is_empty() {
            return;
        }
        let action = serde_json::to_string(&action).unwrap_or_else(|_| "\"\"".to_owned());
        let script = format!(
            "window.rustle && window.rustle.dispatch({{type:'menu.action',action:{action}}});"
        );
        if let Some(chrome) = &state.borrow().chrome {
            let _ = chrome.evaluate_script(&script);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod native_menu {
    use super::{AppState, TabId};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Copy)]
    pub enum Origin {
        Chrome,
        Sidebar,
    }

    pub enum Kind {
        App,
        Tab(TabId),
        Group(u64),
    }

    pub fn show(_state: &Rc<RefCell<AppState>>, _kind: Kind, _origin: Origin, _x: i32, _y: i32) {}
}

#[cfg(target_os = "windows")]
fn start_window_drag(window: &Window) {
    const WM_NCLBUTTONDOWN: u32 = 0x00A1;
    const HTCAPTION: isize = 2;
    let hwnd = window.hwnd();
    unsafe {
        ReleaseCapture();
        SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION, 0);
    }
}

#[cfg(not(target_os = "windows"))]
fn start_window_drag(_window: &Window) {}

fn start_layout_animation(
    state: &Rc<RefCell<AppState>>,
    from_top: u32,
    to_top: u32,
    from_side: u32,
    to_side: u32,
) {
    let proxy = state.borrow().proxy.clone();
    state.borrow_mut().layout_anim = Some(LayoutAnimation {
        started: Instant::now(),
        from_top,
        to_top,
        from_side,
        to_side,
    });
    std::thread::spawn(move || {
        for _ in 0..18 {
            std::thread::sleep(Duration::from_millis(14));
            let _ = proxy.send_event(AppEvent::LayoutTick);
        }
    });
}

fn apply_layout(state: &Rc<RefCell<AppState>>) {
    let (
        width,
        height,
        active,
        overlay_mode,
        dropdown_open,
        dropdown_height,
        find_open,
        vertical,
        sidebar_collapsed,
        sidebar_width_setting,
        prompt_open,
        switcher_open,
    ) = {
        let state = state.borrow();
        let overlay_mode = state.active_tab().is_some_and(|tab| {
            tab.error.is_some() || newtab_overlay(&state) || internal_page(&tab.url).is_some()
        });
        (
            state.width,
            state.height,
            state.active,
            overlay_mode,
            state.dropdown_open,
            state.dropdown_height,
            state.find_open,
            state.vertical,
            state.sidebar_collapsed,
            state.settings.sidebar_width.clamp(72, SIDEBAR_WIDTH),
            state.prompt_open,
            state.switcher_open,
        )
    };

    let target_top = if vertical {
        CHROME_VERTICAL
    } else {
        CHROME_COLLAPSED
    };
    let target_side = if vertical {
        if sidebar_collapsed {
            SIDEBAR_COLLAPSED
        } else {
            sidebar_width_setting
        }
    } else {
        0
    };
    let (top, sidebar_width, sidebar_visible, animation_finished) = {
        let state = state.borrow();
        if let Some(anim) = &state.layout_anim {
            let progress = (anim.started.elapsed().as_millis() as f32 / 252.0).clamp(0.0, 1.0);
            let top = anim.from_top as f32 + (anim.to_top as f32 - anim.from_top as f32) * progress;
            let side =
                anim.from_side as f32 + (anim.to_side as f32 - anim.from_side as f32) * progress;
            (
                top.round() as u32,
                side.round() as u32,
                true,
                progress >= 1.0,
            )
        } else {
            (target_top, target_side, vertical, false)
        }
    };
    let chrome_height = if overlay_mode || prompt_open || switcher_open {
        height
    } else if dropdown_open {
        (dropdown_height + top.saturating_sub(CHROME_COLLAPSED)).clamp(top, height)
    } else if find_open {
        (top + 44).min(height)
    } else {
        top
    };
    let content_width = width.saturating_sub(sidebar_width);

    {
        let state = state.borrow();
        if let Some(chrome) = &state.chrome {
            let _ = chrome.set_bounds(rect(0, 0, width, chrome_height));
        }
        if let Some(sidebar) = &state.sidebar {
            let sidebar_visible = sidebar_visible && !prompt_open && !switcher_open;
            let _ = sidebar.set_visible(sidebar_visible);
            if sidebar_visible {
                let _ = sidebar.set_bounds(rect(0, 0, sidebar_width, height));
            }
        }

        for tab in &state.tabs {
            let Some(webview) = &tab.webview else {
                continue;
            };
            if overlay_mode {
                let _ = webview.set_visible(false);
            } else if tab.id == active {
                let _ = webview.set_visible(true);
                let _ = webview.set_bounds(rect(
                    sidebar_width,
                    top,
                    content_width,
                    height.saturating_sub(top),
                ));
            } else {
                let _ = webview.set_visible(false);
            }
        }
    }

    raise_chrome(state);
    if sidebar_visible && !prompt_open && !switcher_open {
        raise_sidebar(state);
    }
    #[cfg(target_os = "windows")]
    if let Ok(overlays) = RESIZE_OVERLAYS.lock() {
        for hwnd in overlays.iter() {
            if *hwnd != 0 {
                unsafe {
                    SetWindowPos(*hwnd, 0, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
                }
            }
        }
    }
    if animation_finished {
        state.borrow_mut().layout_anim = None;
        let still_vertical = state.borrow().vertical;
        if !still_vertical {
            let sidebar = state.borrow_mut().sidebar.take();
            drop(sidebar);
            focus_active_content(state);
        }
    }
}

fn focus_address(state: &Rc<RefCell<AppState>>) {
    let state = state.borrow();
    if let Some(chrome) = &state.chrome {
        let _ = chrome.focus();
        let _ = chrome.evaluate_script(
            "document.getElementById('address').focus(); \
             document.getElementById('address').select();",
        );
    }
}

fn focus_active_content(state: &Rc<RefCell<AppState>>) {
    let (overlay, newtab) = {
        let state = state.borrow();
        let newtab = newtab_overlay(&state);
        let overlay = state
            .active_tab()
            .is_some_and(|tab| tab.error.is_some() || internal_page(&tab.url).is_some())
            || newtab;
        (overlay, newtab)
    };
    if overlay {
        if newtab {
            focus_address(state);
        } else if let Some(chrome) = &state.borrow().chrome {
            let _ = chrome.focus();
        }
        return;
    }
    let state = state.borrow();
    if let Some(chrome) = &state.chrome {
        let _ = chrome.evaluate_script("document.activeElement && document.activeElement.blur();");
    }
    if let Some(content) = state.active_tab().and_then(|tab| tab.webview.as_ref()) {
        let _ = content.focus();
    }
}

fn on_navigation(state: &Rc<RefCell<AppState>>, tab_id: TabId, url: String) {
    let collapse = {
        let mut state = state.borrow_mut();
        let active = state.active;
        let Some(tab) = state.tab_mut(tab_id) else {
            return;
        };
        let had_error = tab.error.take().is_some();
        tab.url = url;
        tab.favicon = cached_favicon(&tab.url);
        tab.title.clear();
        tab.loading = true;
        tab.cosmetic_generic = true;
        tab.cosmetic_injected.clear();
        tab.cosmetic_exceptions.clear();
        had_error && active == tab_id
    };

    if collapse {
        apply_layout(state);
        focus_active_content(state);
    }
    push_ui_state(state);
    save_session_due(state);

    let proxy = state.borrow().proxy.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(8));
        let _ = proxy.send_event(AppEvent::LoadTimeout(tab_id));
    });
}

fn record_visit(state: &Rc<RefCell<AppState>>, url: &str, title: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    if let Some(storage) = &state.borrow().storage {
        let _ = storage.send(StorageCommand::RecordVisit {
            url: url.to_owned(),
            title: title.to_owned(),
        });
    }
}

fn on_title(state: &Rc<RefCell<AppState>>, tab_id: TabId, title: String) {
    let recorded = {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tab_mut(tab_id) else {
            return;
        };
        if tab.error.is_some() {
            None
        } else {
            tab.title = title;
            if !tab.title.is_empty() {
                tab.loading = false;
            }
            Some((tab.url.clone(), tab.title.clone()))
        }
    };
    if let Some((url, title)) = recorded {
        record_visit(state, &url, &title);
    }
    push_ui_state(state);
    save_session_due(state);
}

fn on_page_load(state: &Rc<RefCell<AppState>>, tab_id: TabId, event: PageLoadEvent) {
    match event {
        PageLoadEvent::Started => {
            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                tab.loading = true;
            }
        }
        PageLoadEvent::Finished => {
            sync_tab_from_webview(state, tab_id);
            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                tab.loading = false;
            }
            request_tab_favicon(state, tab_id);
            let page_url = state
                .borrow()
                .tab(tab_id)
                .map(|tab| tab.url.clone())
                .unwrap_or_default();
            fetch_site_favicon(state, tab_id, &page_url);
            apply_cosmetic_filters(state, tab_id);
            if let Some(webview) = state
                .borrow()
                .tab(tab_id)
                .and_then(|tab| tab.webview.as_ref())
            {
                let _ = webview.evaluate_script(RESIZE_JS);
            }
            let proxy = state.borrow().proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1800));
                let _ = proxy.send_event(AppEvent::RefreshFavicon(tab_id));
            });
        }
    }
    push_ui_state(state);
    save_session_due(state);
}

fn ensure_tab_webview(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    let (url, visible, width, height, top, window, proxy, container) = {
        let state = state.borrow();
        let Some(tab) = state.tab(tab_id) else {
            return;
        };
        if tab.webview.is_some() || is_overlay_url(&tab.url) {
            return;
        }
        (
            tab.url.clone(),
            tab.id == state.active,
            state.width,
            state.height,
            if state.vertical {
                CHROME_VERTICAL
            } else {
                CHROME_COLLAPSED
            },
            state.window.clone(),
            state.proxy.clone(),
            tab.container.clone(),
        )
    };

    let Some(mut context) = state.borrow_mut().context.take() else {
        eprintln!("[tabs] WebContext is missing");
        return;
    };

    let navigation_state = Rc::clone(state);
    let title_state = Rc::clone(state);
    let load_state = Rc::clone(state);
    let new_window_proxy = proxy.clone();

    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_bounds(rect(
            0,
            top,
            width,
            height.saturating_sub(top),
        ))
        .with_visible(visible)
        .with_browser_accelerator_keys(false)
        .with_additional_browser_args(webview_browser_args())
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_navigation_handler(move |url| {
            on_navigation(&navigation_state, tab_id, url);
            true
        })
        .with_document_title_changed_handler(move |title| {
            on_title(&title_state, tab_id, title);
        })
        .with_on_page_load_handler(move |event, _url| {
            on_page_load(&load_state, tab_id, event);
        })
        .with_new_window_req_handler(move |url, _features| {
            let _ = new_window_proxy.send_event(AppEvent::OpenUrl(url));
            NewWindowResponse::Deny
        })
        .with_initialization_script(
            "window.addEventListener('wheel', function (event) { \
               if (event.ctrlKey && window.ipc) { \
                 event.preventDefault(); \
                 window.ipc.postMessage(JSON.stringify({ type: 'zoom.wheel', delta: event.deltaY })); \
               } \
             }, { passive: false }); \
             (function () { \
               if (window.top !== window) { return; } \
               if (window.__rustleAdblockScan) { return; } \
               window.__rustleAdblockScan = true; \
               var sentClasses = new Set(); \
               var sentIds = new Set(); \
               function scan() { \
                 if (!document.body || !window.ipc) { return; } \
                 var classes = []; \
                 var ids = []; \
                 var nodes = document.querySelectorAll('[class],[id]'); \
                 for (var i = 0; i < nodes.length; i++) { \
                   var node = nodes[i]; \
                   var cls = node.classList; \
                   for (var j = 0; cls && j < cls.length; j++) { \
                     if (!sentClasses.has(cls[j])) { sentClasses.add(cls[j]); classes.push(cls[j]); } \
                   } \
                   var id = node.id; \
                   if (id && !sentIds.has(id)) { sentIds.add(id); ids.push(id); } \
                 } \
                 if (classes.length || ids.length) { \
                   window.ipc.postMessage(JSON.stringify({ type: 'adblock.scan', classes: classes, ids: ids })); \
                 } \
               } \
               window.__rustleAdblockRescan = function () { \
                 sentClasses.clear(); sentIds.clear(); scan(); \
               }; \
               if (document.readyState === 'loading') { \
                 document.addEventListener('DOMContentLoaded', scan, { once: true }); \
               } else { scan(); } \
               var timer = 0; \
               new MutationObserver(function () { \
                 clearTimeout(timer); \
                 timer = setTimeout(scan, 250); \
               }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id'] }); \
             })();",
        )
        .with_ipc_handler({
            let proxy = proxy.clone();
            move |request| {
                let _ = proxy.send_event(AppEvent::ContentIpc(tab_id, request.body().to_owned()));
            }
        })
        .with_url(&url);

    #[cfg(target_os = "windows")]
    let builder = if let Some(container) = container.as_deref() {
        builder.with_profile_name(format!("rustle-{container}"))
    } else {
        builder
    };
    let result = builder.build_as_child(window.as_ref());
    state.borrow_mut().context = Some(context);

    let webview = match result {
        Ok(webview) => webview,
        Err(error) => {
            eprintln!("[tabs] failed to create WebView: {error}");
            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                tab.loading = false;
                tab.error = Some(ErrorInfo {
                    code: "render".to_owned(),
                    message: "Не удалось создать вкладку.".to_owned(),
                });
            }
            push_ui_state(state);
            return;
        }
    };

    if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
        let zoom = tab.zoom;
        tab.webview = Some(webview);
        #[cfg(target_os = "windows")]
        if let Some(webview) = tab.webview.as_ref() {
            attach_resize_passthrough(webview);
            unsafe {
                let _ = webview.controller().SetZoomFactor(zoom);
            }
            #[cfg(target_os = "windows")]
            apply_webview_muted(webview, tab.muted);
        }
    }

    {
        let state_ref = state.borrow();
        if let Some(webview) = state_ref.tab(tab_id).and_then(|tab| tab.webview.as_ref()) {
            install_tab_navigation_handlers(state, webview, tab_id, proxy);
            install_download_handler(state, webview, tab_id);
            install_permission_handler(state, webview);
            install_crash_handler(state, webview, tab_id);
            install_zoom_handler(state, webview, tab_id);
            install_adblock_handler(state, webview, tab_id);
            if !state.borrow().private {
                install_favicon_handler(state, webview, tab_id);
            }
            install_accelerators_for_view(state, webview);
        }
    }

    raise_chrome(state);
}

fn navigate_active(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(target) = resolve_input(input) else {
        return;
    };
    let (engine, https_first) = {
        let state = state.borrow();
        (
            state.settings.search_engine.clone(),
            state.settings.https_first,
        )
    };
    let url = match target {
        rustle_core::NavigationTarget::Url(url) => {
            maybe_upgrade_https(canonicalize_internal(url), https_first)
        }
        rustle_core::NavigationTarget::Search { query } => search_url(&engine, &query),
    };

    let active = state.borrow().active;
    let (previous_url, previous_title) = {
        let state = state.borrow();
        let Some(tab) = state.tab(active) else {
            return;
        };
        (tab.url.clone(), effective_tab_title(tab))
    };

    if is_overlay_url(&url) {
        {
            let mut state = state.borrow_mut();
            let Some(tab) = state.tab_mut(active) else {
                return;
            };
            if !is_overlay_url(&previous_url) && !previous_url.is_empty() {
                tab.return_url = Some(previous_url);
                tab.return_title = Some(previous_title);
            }
            tab.url = url.clone();
            tab.title = fallback_title(&url);
            tab.loading = false;
            tab.error = None;
        }

        apply_layout(state);
        focus_active_content(state);
        push_ui_state(state);
        save_session_state(state, false);
        return;
    }

    {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tab_mut(active) else {
            return;
        };
        tab.return_url = None;
        tab.url = url.clone();
        tab.title.clear();
        tab.loading = true;
        tab.error = None;
    }

    ensure_tab_webview(state, active);
    let loaded = {
        let state = state.borrow();
        state
            .tab(active)
            .and_then(|tab| tab.webview.as_ref())
            .map(|webview| webview.load_url(&url).is_ok())
            .unwrap_or(false)
    };
    if !loaded {
        if let Some(tab) = state.borrow_mut().tab_mut(active) {
            tab.loading = false;
        }
    }

    apply_layout(state);
    let zoom = state
        .borrow()
        .active_tab()
        .map(|tab| tab.zoom)
        .unwrap_or(1.0);
    apply_zoom(state, zoom);
    focus_active_content(state);
    push_ui_state(state);
    save_session_state(state, false);
}

fn reload_active(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    let (url, error, has_webview, overlay) = {
        let state = state.borrow();
        let Some(tab) = state.tab(active) else {
            return;
        };
        (
            tab.url.clone(),
            tab.error.is_some(),
            tab.webview.is_some(),
            is_overlay_url(&tab.url),
        )
    };

    if error {
        navigate_active(state, &url);
        return;
    }
    if !has_webview || overlay {
        return;
    }

    let reloaded = {
        let state = state.borrow();
        state
            .tab(active)
            .and_then(|tab| tab.webview.as_ref())
            .map(|webview| webview.reload().is_ok())
            .unwrap_or(false)
    };
    if reloaded {
        if let Some(tab) = state.borrow_mut().tab_mut(active) {
            tab.loading = true;
        }
        push_ui_state(state);
    }
}

fn stop_active(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    {
        let state = state.borrow();
        if let Some(webview) = state.tab(active).and_then(|tab| tab.webview.as_ref()) {
            let _ = webview.evaluate_script("window.stop()");
        }
    }
    if let Some(tab) = state.borrow_mut().tab_mut(active) {
        tab.loading = false;
    }
    push_ui_state(state);
}

fn go_back_active(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    let overlay_return = {
        let state = state.borrow();
        let Some(tab) = state.tab(active) else {
            return;
        };
        if is_overlay_url(&tab.url) {
            tab.return_url.clone().map(|url| {
                let restorable = tab.webview.is_some()
                    && tab.error.is_none()
                    && !url.is_empty()
                    && !is_overlay_url(&url);
                (url, restorable)
            })
        } else {
            None
        }
    };
    if let Some((url, restorable)) = overlay_return {
        if restorable {
            // The content WebView is still alive underneath the internal page.
            // Bring it back without navigating: re-loading it here used to leave
            // the tab spinner running forever when the site was restored.
            {
                let mut state = state.borrow_mut();
                let Some(tab) = state.tab_mut(active) else {
                    return;
                };
                let restored_title = tab
                    .return_title
                    .take()
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| fallback_title(&url));
                tab.return_url = None;
                tab.url = url;
                tab.title = restored_title;
                tab.loading = false;
                tab.error = None;
            }
            sync_tab_from_webview(state, active);
            apply_layout(state);
            focus_active_content(state);
            push_ui_state(state);
            save_session_state(state, false);
            return;
        }
        navigate_active(state, &url);
        return;
    }
    if state
        .borrow()
        .tab(active)
        .is_some_and(|tab| !tab.can_back || tab.error.is_some() || tab.webview.is_none())
    {
        return;
    }
    let moved = {
        let state = state.borrow();
        state
            .tab(active)
            .and_then(|tab| tab.webview.as_ref())
            .map(|webview| webview.go_back().is_ok())
            .unwrap_or(false)
    };
    if moved {
        if let Some(tab) = state.borrow_mut().tab_mut(active) {
            tab.loading = true;
        }
        push_ui_state(state);
    }
}

fn go_forward_active(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    if state
        .borrow()
        .tab(active)
        .is_some_and(|tab| !tab.can_forward || tab.error.is_some() || tab.webview.is_none())
    {
        return;
    }
    let moved = {
        let state = state.borrow();
        state
            .tab(active)
            .and_then(|tab| tab.webview.as_ref())
            .map(|webview| webview.go_forward().is_ok())
            .unwrap_or(false)
    };
    if moved {
        if let Some(tab) = state.borrow_mut().tab_mut(active) {
            tab.loading = true;
        }
        push_ui_state(state);
    }
}

fn open_devtools_active(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    let state = state.borrow();
    if let Some(webview) = state.tab(active).and_then(|tab| tab.webview.as_ref()) {
        webview.open_devtools();
    }
}

#[cfg(target_os = "windows")]
fn install_tab_navigation_handlers(
    state: &Rc<RefCell<AppState>>,
    webview: &WebView,
    tab_id: TabId,
    proxy: EventLoopProxy<AppEvent>,
) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
        },
        NavigationCompletedEventHandler, NavigationStartingEventHandler,
    };
    use windows_core::BOOL;

    let start_state = Rc::clone(state);
    let mut start_token = 0i64;
    let start_handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut navigation_id = 0u64;
        unsafe {
            let _ = args.NavigationId(&mut navigation_id);
        }
        if let Some(tab) = start_state.borrow_mut().tab_mut(tab_id) {
            tab.nav_id = navigation_id;
        }
        Ok(())
    }));

    unsafe {
        if let Err(error) = webview
            .webview()
            .add_NavigationStarting(&start_handler, &mut start_token)
        {
            eprintln!("[tabs] failed to register starting handler: {error}");
        }
    }

    let state = Rc::clone(state);
    let mut token = 0i64;
    let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };

        let mut navigation_id = 0u64;
        unsafe {
            let _ = args.NavigationId(&mut navigation_id);
        }
        let current_id = state
            .borrow()
            .tab(tab_id)
            .map(|tab| tab.nav_id)
            .unwrap_or(0);
        if current_id != 0 && navigation_id != 0 && navigation_id != current_id {
            return Ok(());
        }

        let mut success = BOOL(0);
        unsafe {
            let _ = args.IsSuccess(&mut success);
        }

        if success.as_bool() {
            let active = state.borrow().active;
            let had_error = state
                .borrow()
                .tab(tab_id)
                .is_some_and(|tab| tab.error.is_some());

            if had_error {
                if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                    tab.error = None;
                    tab.title.clear();
                }
                if active == tab_id {
                    apply_layout(&state);
                    focus_active_content(&state);
                }
            }

            sync_tab_from_webview(&state, tab_id);
            let visited = {
                if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                    tab.loading = false;
                    Some((tab.url.clone(), tab.title.clone()))
                } else {
                    None
                }
            };
            if let Some((url, title)) = visited {
                record_visit(&state, &url, &title);
            }
            push_ui_state(&state);

            let state_ref = state.borrow();
            if let Some(content) = state_ref.tab(tab_id).and_then(|tab| tab.webview.as_ref()) {
                let proxy = proxy.clone();
                let _ = content.evaluate_script_with_callback("document.title", move |title| {
                    let title = serde_json::from_str::<String>(&title).unwrap_or(title);
                    let _ = proxy.send_event(AppEvent::Title(tab_id, title));
                });
            }
            return Ok(());
        }

        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
        unsafe {
            let _ = args.WebErrorStatus(&mut status);
        }

        if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                tab.loading = false;
            }
            push_ui_state(&state);
            return Ok(());
        }

        let error = error_info(status);
        eprintln!("[nav] tab {} failed: {}", tab_id.0, error.code);
        set_tab_error(&state, tab_id, error);
        Ok(())
    }));

    unsafe {
        if let Err(error) = webview
            .webview()
            .add_NavigationCompleted(&handler, &mut token)
        {
            eprintln!("[tabs] failed to register completion handler: {error}");
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn install_tab_navigation_handlers(
    _state: &Rc<RefCell<AppState>>,
    _webview: &WebView,
    _tab_id: TabId,
    _proxy: EventLoopProxy<AppEvent>,
) {
}

fn set_tab_error(state: &Rc<RefCell<AppState>>, tab_id: TabId, error: ErrorInfo) {
    let active = state.borrow().active;
    {
        let mut state = state.borrow_mut();
        if let Some(tab) = state.tab_mut(tab_id) {
            tab.error = Some(error);
            tab.loading = false;
            tab.title = "Не удалось открыть страницу".to_owned();
        }
    }

    if active == tab_id {
        apply_layout(state);
        if let Some(chrome) = &state.borrow().chrome {
            let _ = chrome.focus();
        }
    }
    push_ui_state(state);
}

#[cfg(target_os = "windows")]
fn error_info(
    status: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS,
) -> ErrorInfo {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;

    let (code, message) = if status == COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED {
        (
            "dns",
            "Не удалось найти сайт. Проверьте адрес и подключение к интернету.",
        )
    } else if status == COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED {
        ("offline", "Нет подключения к интернету.")
    } else if status == COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE
    {
        ("connect", "Не удалось подключиться к сайту.")
    } else if status == COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT {
        ("timeout", "Сайт слишком долго не отвечает.")
    } else if status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
    {
        (
            "tls",
            "Проблема с сертификатом сайта. Соединение небезопасно.",
        )
    } else if status == COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED
        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET
    {
        ("connect", "Соединение с сайтом прервано.")
    } else {
        ("other", "Не удалось открыть страницу.")
    };

    ErrorInfo {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(target_os = "windows")]
fn apply_tab_memory_levels(state: &Rc<RefCell<AppState>>) {
    let state_ref = state.borrow();
    for tab in &state_ref.tabs {
        if let Some(webview) = &tab.webview {
            let level = if tab.id == state_ref.active {
                MemoryUsageLevel::Normal
            } else {
                MemoryUsageLevel::Low
            };
            let _ = webview.set_memory_usage_level(level);
        }
    }
}

fn activate_tab(state: &Rc<RefCell<AppState>>, id: TabId) {
    let exists = state.borrow().tab(id).is_some();
    if !exists {
        return;
    }

    let previous = state.borrow().active;
    if previous == id {
        ensure_tab_webview(state, id);
        #[cfg(target_os = "windows")]
        apply_tab_memory_levels(state);
        apply_layout(state);
        focus_active_content(state);
        push_ui_state(state);
        return;
    }

    {
        let state = state.borrow();
        if let Some(webview) = state.tab(previous).and_then(|tab| tab.webview.as_ref()) {
            let _ = webview.set_visible(false);
        }
    }

    {
        let mut state_ref = state.borrow_mut();
        state_ref.active = id;
        if let Some(tab) = state_ref.tab_mut(id) {
            tab.last_active = Instant::now();
        }
    }
    ensure_tab_webview(state, id);
    #[cfg(target_os = "windows")]
    apply_tab_memory_levels(state);
    apply_layout(state);
    focus_active_content(state);
    push_ui_state(state);
    save_session_state(state, false);
}

fn close_tab(state: &Rc<RefCell<AppState>>, id: TabId) {
    let (index, was_active) = {
        let state = state.borrow();
        let Some(index) = state.tab_index(id) else {
            return;
        };
        (index, state.active == id)
    };

    let removed = {
        let mut state = state.borrow_mut();
        state.tabs.remove(index)
    };

    {
        let mut state = state.borrow_mut();
        state.closed.push(ClosedTab {
            url: removed.url.clone(),
            title: removed.title.clone(),
        });
        if state.closed.len() > MAX_CLOSED_TABS {
            state.closed.remove(0);
        }
    }
    drop(removed);

    if state.borrow().tabs.is_empty() {
        let _ = state.borrow().proxy.send_event(AppEvent::Exit);
        return;
    }

    if was_active {
        let next_index = index.min(state.borrow().tabs.len().saturating_sub(1));
        let next_id = state.borrow().tabs[next_index].id;
        state.borrow_mut().active = next_id;
        ensure_tab_webview(state, next_id);
        apply_layout(state);
        focus_active_content(state);
        push_ui_state(state);
    } else {
        apply_layout(state);
        push_ui_state(state);
    }
    save_session_state(state, false);
}

fn new_tab(state: &Rc<RefCell<AppState>>) {
    let id = {
        let mut state = state.borrow_mut();
        push_tab(
            &mut state,
            NEWTAB_URL.to_owned(),
            "Новая вкладка".to_owned(),
        )
    };
    activate_tab(state, id);
    focus_address(state);
}

#[cfg(target_os = "windows")]
fn apply_webview_muted(webview: &WebView, muted: bool) {
    if let Ok(webview) = webview.webview().cast::<ICoreWebView2_8>() {
        unsafe {
            let _ = webview.SetIsMuted(muted);
        }
    }
}

fn toggle_tab_muted(state: &Rc<RefCell<AppState>>, id: TabId) {
    let muted = {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tab_mut(id) else {
            return;
        };
        tab.muted = !tab.muted;
        tab.muted
    };
    #[cfg(target_os = "windows")]
    if let Some(webview) = state.borrow().tab(id).and_then(|tab| tab.webview.as_ref()) {
        apply_webview_muted(webview, muted);
    }
    push_ui_state(state);
    save_session_due(state);
}

fn set_tab_pinned(state: &Rc<RefCell<AppState>>, id: TabId, pinned: bool) {
    {
        let mut state = state.borrow_mut();
        let Some(index) = state.tab_index(id) else {
            return;
        };
        if state.tabs[index].pinned == pinned {
            return;
        }
        let mut tab = state.tabs.remove(index);
        tab.pinned = pinned;
        if pinned {
            tab.group = None;
        }
        let pinned_count = state.tabs.iter().filter(|entry| entry.pinned).count();
        let insert_at = pinned_count.min(state.tabs.len());
        state.tabs.insert(insert_at, tab);
    }
    push_ui_state(state);
    apply_layout(state);
    save_session_state(state, false);
}

fn rename_group(state: &Rc<RefCell<AppState>>, group_id: u64, name: String) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    {
        let mut state = state.borrow_mut();
        let Some(group) = state.groups.iter_mut().find(|entry| entry.id == group_id) else {
            return;
        };
        group.name = name.to_owned();
    }
    push_ui_state(state);
    save_session_due(state);
}

fn tab_group_collapsed(groups: &[TabGroup], group: Option<u64>) -> bool {
    group
        .and_then(|id| groups.iter().find(|entry| entry.id == id))
        .is_some_and(|entry| entry.collapsed)
}

fn create_group(state: &Rc<RefCell<AppState>>, name: String, tab: Option<TabId>) {
    {
        let mut state = state.borrow_mut();
        let id = state.next_group_id;
        state.next_group_id += 1;
        state.groups.push(TabGroup {
            id,
            hue: ((id.wrapping_mul(47)) % 360) as u16,
            name: if name.trim().is_empty() {
                "Группа".to_owned()
            } else {
                name.trim().to_owned()
            },
            collapsed: false,
        });
        if let Some(tab_id) = tab {
            if let Some(tab) = state.tab_mut(tab_id) {
                tab.group = Some(id);
            }
        }
    }
    push_ui_state(state);
    save_session_due(state);
}

fn toggle_group(state: &Rc<RefCell<AppState>>, group_id: u64) {
    let (collapsed, active_inside) = {
        let state = state.borrow();
        let Some(group) = state.groups.iter().find(|entry| entry.id == group_id) else {
            return;
        };
        (
            !group.collapsed,
            state
                .active_tab()
                .is_some_and(|tab| tab.group == Some(group_id)),
        )
    };
    if collapsed && active_inside {
        let fallback = {
            let state = state.borrow();
            state
                .tabs
                .iter()
                .find(|tab| {
                    tab.group != Some(group_id) && !tab_group_collapsed(&state.groups, tab.group)
                })
                .map(|tab| tab.id)
        };
        let Some(fallback) = fallback else {
            return;
        };
        activate_tab(state, fallback);
    }
    let mut detached = Vec::new();
    {
        let mut state = state.borrow_mut();
        if let Some(group) = state.groups.iter_mut().find(|entry| entry.id == group_id) {
            group.collapsed = collapsed;
        }
        if collapsed {
            for tab in state
                .tabs
                .iter_mut()
                .filter(|tab| tab.group == Some(group_id))
            {
                if let Some(webview) = tab.webview.take() {
                    detached.push(webview);
                }
                tab.loading = false;
            }
        }
    }
    drop(detached);
    push_ui_state(state);
    save_session_due(state);
}

fn remove_group(state: &Rc<RefCell<AppState>>, group_id: u64) {
    let tab_ids: Vec<TabId> = {
        let state = state.borrow();
        state
            .tabs
            .iter()
            .filter(|tab| tab.group == Some(group_id))
            .map(|tab| tab.id)
            .collect()
    };
    {
        let mut state = state.borrow_mut();
        state.groups.retain(|group| group.id != group_id);
    }
    for tab_id in tab_ids {
        close_tab(state, tab_id);
    }
    push_ui_state(state);
    save_session_due(state);
}

fn new_tab_in_group(state: &Rc<RefCell<AppState>>, group_id: u64) {
    let id = {
        let mut state = state.borrow_mut();
        if !state.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        if let Some(group) = state.groups.iter_mut().find(|group| group.id == group_id) {
            group.collapsed = false;
        }
        let id = push_tab(
            &mut state,
            NEWTAB_URL.to_owned(),
            "Новая вкладка".to_owned(),
        );
        if let Some(tab) = state.tab_mut(id) {
            tab.group = Some(group_id);
        }
        id
    };
    activate_tab(state, id);
    focus_address(state);
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedSessionFile {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    saved_at: i64,
    session: Session,
}

fn sessions_dir() -> PathBuf {
    rustle_storage::data_dir().join("sessions")
}

fn refresh_saved_sessions(state: &Rc<RefCell<AppState>>) {
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(sessions_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(saved) = serde_json::from_str::<SavedSessionFile>(&text) else {
                continue;
            };
            let id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let preview: Vec<String> = saved
                .session
                .tabs
                .iter()
                .take(4)
                .map(|tab| tab.title.clone())
                .collect();
            items.push(json!({
                "id": id,
                "name": saved.name,
                "savedAt": saved.saved_at,
                "count": saved.session.tabs.len(),
                "preview": preview,
                "tags": saved.tags,
            }));
        }
    }
    items.sort_by_key(|item| {
        item.get("savedAt")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
    });
    items.reverse();
    state.borrow_mut().saved_sessions = items;
    push_ui_state(state);
}

fn suspend_idle_tabs(state: &Rc<RefCell<AppState>>) {
    let minutes = state.borrow().settings.auto_suspend_minutes;
    if minutes == 0 {
        return;
    }
    let active = state.borrow().active;
    let limit = Duration::from_secs(minutes as u64 * 60);
    let mut suspended = 0usize;
    {
        let mut state_ref = state.borrow_mut();
        for tab in &mut state_ref.tabs {
            if tab.id != active
                && !tab.pinned
                && tab.webview.is_some()
                && tab.last_active.elapsed() >= limit
            {
                let _ = tab.webview.take();
                tab.loading = false;
                suspended += 1;
            }
        }
    }
    if suspended > 0 {
        push_ui_state(state);
        save_session_due(state);
    }
}

fn unload_inactive_tabs(state: &Rc<RefCell<AppState>>) {
    let active = state.borrow().active;
    {
        let mut state_ref = state.borrow_mut();
        for tab in &mut state_ref.tabs {
            if tab.id != active && !tab.pinned {
                let _ = tab.webview.take();
                tab.loading = false;
            }
        }
    }
    push_ui_state(state);
    save_session_due(state);
}

fn save_named_session(state: &Rc<RefCell<AppState>>, name: &str, tags: &[String]) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    save_session_state(state, false);
    let Ok(text) = fs::read_to_string(rustle_storage::session_path()) else {
        return;
    };
    let Ok(session) = serde_json::from_str::<Session>(&text) else {
        return;
    };
    let _ = fs::create_dir_all(sessions_dir());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let safe: String = name
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let id = format!("{safe}-{stamp}");
    let payload = SavedSessionFile {
        name: name.to_owned(),
        tags: tags.to_vec(),
        saved_at: stamp as i64,
        session,
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&payload) {
        let _ = fs::write(sessions_dir().join(format!("{id}.json")), bytes);
    }
    refresh_saved_sessions(state);
}

fn load_named_session(state: &Rc<RefCell<AppState>>, id: &str) {
    if !id
        .chars()
        .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_')
    {
        return;
    }
    let path = sessions_dir().join(format!("{id}.json"));
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(saved) = serde_json::from_str::<SavedSessionFile>(&text) else {
        return;
    };
    {
        let mut state_ref = state.borrow_mut();
        state_ref.tabs.clear();
        state_ref.closed.clear();
        state_ref.groups.clear();
        state_ref.active = TabId(0);
    }
    restore_session(state, Some(saved.session));
    let active = state.borrow().active;
    ensure_tab_webview(state, active);
    apply_layout(state);
    push_ui_state(state);
}

fn delete_named_session(state: &Rc<RefCell<AppState>>, id: &str) {
    if !id
        .chars()
        .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_')
    {
        return;
    }
    let _ = fs::remove_file(sessions_dir().join(format!("{id}.json")));
    refresh_saved_sessions(state);
}

fn save_session_as(state: &Rc<RefCell<AppState>>) {
    save_session_state(state, false);
    let Some(target) = rfd::FileDialog::new()
        .set_title("Сохранить сессию Rustle")
        .add_filter("Rustle session", &["json"])
        .set_file_name("rustle-session.json")
        .save_file()
    else {
        return;
    };
    let _ = fs::copy(rustle_storage::session_path(), target);
}

fn open_session_as(state: &Rc<RefCell<AppState>>) {
    let Some(source) = rfd::FileDialog::new()
        .set_title("Открыть сессию Rustle")
        .add_filter("Rustle session", &["json"])
        .pick_file()
    else {
        return;
    };
    let Ok(text) = fs::read(source) else {
        return;
    };
    let Ok(session) = serde_json::from_slice::<Session>(&text) else {
        return;
    };
    {
        let mut state_ref = state.borrow_mut();
        state_ref.tabs.clear();
        state_ref.closed.clear();
        state_ref.groups.clear();
        state_ref.active = TabId(0);
    }
    restore_session(state, Some(session));
    let active = state.borrow().active;
    ensure_tab_webview(state, active);
    apply_layout(state);
    push_ui_state(state);
}

#[allow(dead_code)]
fn check_updates(state: &Rc<RefCell<AppState>>) {
    let proxy = state.borrow().proxy.clone();
    std::thread::spawn(move || {
        let status =
            match ureq::get("https://api.github.com/repos/d1m40fficial/rustle/releases/latest")
                .set("User-Agent", "Rustle")
                .call()
            {
                Ok(response) => match response
                    .into_string()
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                {
                    Some(value) => {
                        let latest = value
                            .get("tag_name")
                            .and_then(|tag| tag.as_str())
                            .unwrap_or("")
                            .trim_start_matches('v')
                            .to_owned();
                        if latest.is_empty() {
                            "Не удалось проверить обновления".to_owned()
                        } else if latest != VERSION {
                            format!("Доступна новая версия {latest} (у вас {VERSION})")
                        } else {
                            format!("У вас последняя версия {VERSION}")
                        }
                    }
                    None => "Не удалось проверить обновления".to_owned(),
                },
                Err(_) => "Не удалось проверить обновления".to_owned(),
            };
        let _ = proxy.send_event(AppEvent::UpdateStatus(status));
    });
}

fn screenshot_window(state: &Rc<RefCell<AppState>>) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Сохранить скриншот страницы")
        .add_filter("PNG", &["png"])
        .set_file_name("rustle-screenshot.png")
        .save_file()
    else {
        return;
    };
    let hwnd = {
        let state = state.borrow();
        state
            .active_tab()
            .and_then(|tab| tab.webview.as_ref())
            .map(|view| view.hwnd().0 as isize)
    };
    let Some(hwnd) = hwnd else {
        return;
    };
    let mut rect = ResizeRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetWindowRect(hwnd, &mut rect);
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }
    let escaped = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; Add-Type -AssemblyName System.Drawing; $bmp=New-Object System.Drawing.Bitmap {width},{height}; $g=[System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen({left},{top},0,0,(New-Object System.Drawing.Size {width},{height})); $bmp.Save('{escaped}',[System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()",
        left = rect.left,
        top = rect.top
    );
    let _ = ProcessCommand::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .spawn();
}

fn print_active_pdf(state: &Rc<RefCell<AppState>>) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Сохранить страницу в PDF")
        .add_filter("PDF", &["pdf"])
        .set_file_name("rustle-page.pdf")
        .save_file()
    else {
        return;
    };
    let Some(webview) = state
        .borrow()
        .active_tab()
        .and_then(|tab| tab.webview.as_ref().map(|view| view.webview()))
    else {
        return;
    };
    #[cfg(target_os = "windows")]
    if let Ok(pdf) = webview.cast::<ICoreWebView2_7>() {
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let _ = pdf.PrintToPdf(
                PCWSTR(wide.as_ptr()),
                None,
                None::<&ICoreWebView2PrintToPdfCompletedHandler>,
            );
        }
    }
}

fn whitelist_current(state: &Rc<RefCell<AppState>>) {
    let host = state
        .borrow()
        .active_tab()
        .and_then(|tab| Url::parse(&tab.url).ok())
        .and_then(|url| url.host_str().map(str::to_lowercase));
    let Some(host) = host else {
        return;
    };
    {
        let mut state_ref = state.borrow_mut();
        if !state_ref
            .settings
            .adblock_exceptions
            .iter()
            .any(|entry| entry == &host)
        {
            state_ref.settings.adblock_exceptions.push(host);
        }
        if !state_ref.private {
            let _ = rustle_storage::save_settings(&state_ref.settings);
        }
    }
    push_ui_state(state);
    reload_active(state);
}

fn export_config(state: &Rc<RefCell<AppState>>) {
    let (settings, groups) = {
        let state = state.borrow();
        (
            state.settings.clone(),
            state.groups.iter().map(|group| {
                json!({"id": group.id, "name": group.name, "collapsed": group.collapsed, "hue": group.hue})
            }).collect::<Vec<_>>(),
        )
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("Экспорт настроек Rustle")
        .add_filter("JSON", &["json"])
        .set_file_name("rustle-settings.json")
        .save_file()
    else {
        return;
    };
    let payload = json!({"version": 1, "settings": settings, "groups": groups});
    if let Ok(text) = serde_json::to_string_pretty(&payload) {
        let _ = fs::write(path, text);
    }
}

fn import_config(state: &Rc<RefCell<AppState>>) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Импорт настроек Rustle")
        .add_filter("JSON", &["json"])
        .pick_file()
    else {
        return;
    };
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if let Some(settings) = value
        .get("settings")
        .and_then(|value| serde_json::from_value::<Settings>(value.clone()).ok())
    {
        if !state.borrow().private {
            let _ = rustle_storage::save_settings(&settings);
        }
        state.borrow_mut().settings = settings;
    }
    if let Some(groups) = value.get("groups").and_then(|value| value.as_array()) {
        let mut state = state.borrow_mut();
        state.groups = groups
            .iter()
            .filter_map(|group| {
                Some(TabGroup {
                    id: group.get("id")?.as_u64()?,
                    name: group.get("name")?.as_str()?.to_owned(),
                    collapsed: group
                        .get("collapsed")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                    hue: group
                        .get("hue")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(24) as u16,
                })
            })
            .collect();
        state.next_group_id = state.groups.iter().map(|group| group.id).max().unwrap_or(0) + 1;
    }
    apply_layout(state);
    push_ui_state(state);
}

fn toggle_all_groups(state: &Rc<RefCell<AppState>>) {
    {
        let mut state = state.borrow_mut();
        if state.groups.is_empty() {
            return;
        }
        let collapse = state.groups.iter().any(|group| !group.collapsed);
        for group in &mut state.groups {
            group.collapsed = collapse;
        }
    }
    push_ui_state(state);
    save_session_due(state);
    apply_layout(state);
}

fn cycle_group(state: &Rc<RefCell<AppState>>, delta: i32) {
    let target = {
        let state_ref = state.borrow();
        if state_ref.groups.is_empty() {
            return;
        }
        let active_group = state_ref
            .active_tab()
            .and_then(|tab| tab.group)
            .and_then(|id| state_ref.groups.iter().position(|group| group.id == id));
        let current = active_group.unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(state_ref.groups.len() as i32) as usize;
        state_ref.groups[next].id
    };
    let target_tab = {
        let state_ref = state.borrow();
        state_ref
            .tabs
            .iter()
            .find(|tab| tab.group == Some(target))
            .map(|tab| tab.id)
    };
    if let Some(id) = target_tab {
        {
            let mut state_ref = state.borrow_mut();
            if let Some(group) = state_ref.groups.iter_mut().find(|group| group.id == target) {
                group.collapsed = false;
            }
        }
        activate_tab(state, id);
    }
}

fn auto_group_by_domain(state: &Rc<RefCell<AppState>>) {
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<TabId>> = HashMap::new();
    {
        let state_ref = state.borrow();
        for tab in &state_ref.tabs {
            if tab.pinned || tab.group.is_some() {
                continue;
            }
            let Ok(url) = Url::parse(&tab.url) else {
                continue;
            };
            let Some(host) = url.host_str() else {
                continue;
            };
            groups
                .entry(host.trim_start_matches("www.").to_owned())
                .or_default()
                .push(tab.id);
        }
    }
    let mut changed = false;
    for (host, tabs) in groups {
        if tabs.len() < 2 {
            continue;
        }
        let group_id = {
            let mut state = state.borrow_mut();
            if let Some(group) = state.groups.iter().find(|group| group.name == host) {
                group.id
            } else {
                let id = state.next_group_id;
                state.next_group_id += 1;
                let hue = (host.bytes().fold(0u32, |acc, byte| {
                    acc.wrapping_mul(31).wrapping_add(byte as u32)
                }) % 360) as u16;
                state.groups.push(TabGroup {
                    id,
                    name: host.clone(),
                    collapsed: false,
                    hue,
                });
                id
            }
        };
        let mut state = state.borrow_mut();
        for tab_id in tabs {
            if let Some(tab) = state.tab_mut(tab_id) {
                tab.group = Some(group_id);
                changed = true;
            }
        }
    }
    if changed {
        push_ui_state(state);
        save_session_due(state);
    }
}

fn set_tab_container(state: &Rc<RefCell<AppState>>, id: TabId, container: &str) {
    {
        let mut state_ref = state.borrow_mut();
        let Some(tab) = state_ref.tab_mut(id) else {
            return;
        };
        let next = if container == "none" {
            None
        } else {
            Some(container.to_owned())
        };
        if tab.container == next {
            return;
        }
        tab.container = next;
        let _ = tab.webview.take();
    }
    push_ui_state(state);
    ensure_tab_webview(state, id);
    save_session_due(state);
}

struct ResizeSession {
    edge: i32,
    start_x: i32,
    start_y: i32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn resize_window_ipc(state: &Rc<RefCell<AppState>>, message: &Value, start: bool, end: bool) {
    #[cfg(target_os = "windows")]
    {
        let hwnd = state.borrow().window.hwnd();
        if end {
            state.borrow_mut().resize_session = None;
            return;
        }
        let screen_x = message
            .get("screenX")
            .and_then(|value| value.as_i64())
            .unwrap_or(0) as i32;
        let screen_y = message
            .get("screenY")
            .and_then(|value| value.as_i64())
            .unwrap_or(0) as i32;
        if start {
            let mut rect = ResizeRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            unsafe {
                GetWindowRect(hwnd, &mut rect);
            }
            let edge = message
                .get("edge")
                .and_then(|value| value.as_i64())
                .unwrap_or(0) as i32;
            state.borrow_mut().resize_session = Some(ResizeSession {
                edge,
                start_x: screen_x,
                start_y: screen_y,
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            });
            let lparam = ((screen_y as isize) << 16) | (screen_x as isize & 0xffff);
            unsafe {
                ReleaseCapture();
                SendMessageW(hwnd, 0x00A1, edge as isize, lparam);
            }
            state.borrow_mut().resize_session = None;
        } else if let Some(session) = &state.borrow().resize_session {
            let dx = screen_x - session.start_x;
            let dy = screen_y - session.start_y;
            let mut left = session.left;
            let mut top = session.top;
            let mut right = session.right;
            let mut bottom = session.bottom;
            if matches!(session.edge, 10 | 13 | 16) {
                left += dx;
            }
            if matches!(session.edge, 11 | 14 | 17) {
                right += dx;
            }
            if matches!(session.edge, 12..=14) {
                top += dy;
            }
            if matches!(session.edge, 15..=17) {
                bottom += dy;
            }
            if right - left < 720 {
                if matches!(session.edge, 10 | 13 | 16) {
                    left = right - 720;
                } else {
                    right = left + 720;
                }
            }
            if bottom - top < 480 {
                if matches!(session.edge, 12..=14) {
                    top = bottom - 480;
                } else {
                    bottom = top + 480;
                }
            }
            unsafe {
                SetWindowPos(
                    hwnd,
                    0,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    0x0004 | 0x0010,
                );
            }
        }
    }
}

fn move_tab(state: &Rc<RefCell<AppState>>, tab_id: TabId, target_id: TabId) {
    if tab_id == target_id {
        return;
    }
    {
        let mut state = state.borrow_mut();
        let Some(from) = state.tab_index(tab_id) else {
            return;
        };
        let tab = state.tabs.remove(from);
        let Some(to) = state.tab_index(target_id) else {
            let position = from.min(state.tabs.len());
            state.tabs.insert(position, tab);
            return;
        };
        state.tabs.insert(to, tab);
        state.tabs.sort_by_key(|entry| !entry.pinned);
    }
    push_ui_state(state);
    save_session_due(state);
}

fn move_tab_to_group(state: &Rc<RefCell<AppState>>, tab_id: TabId, group_id: Option<u64>) {
    let changed = {
        let mut state = state.borrow_mut();
        if let Some(group_id) = group_id {
            if !state.groups.iter().any(|group| group.id == group_id) {
                return;
            }
            if let Some(group) = state.groups.iter_mut().find(|group| group.id == group_id) {
                group.collapsed = false;
            }
        }
        if let Some(tab) = state.tab_mut(tab_id) {
            tab.group = group_id;
            true
        } else {
            false
        }
    };
    if changed {
        push_ui_state(state);
        save_session_due(state);
    }
}

fn open_external_url(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(target) = resolve_input(input) else {
        return;
    };
    let url = match target {
        rustle_core::NavigationTarget::Url(url) => {
            let settings = state.borrow().settings.clone();
            maybe_upgrade_https(canonicalize_internal(url), settings.https_first)
        }
        rustle_core::NavigationTarget::Search { query } => {
            search_url(&state.borrow().settings.search_engine, &query)
        }
    };
    open_tab(state, url);
}

fn open_tab(state: &Rc<RefCell<AppState>>, url: String) {
    let url = canonicalize_internal(url);
    let title = fallback_title(&url);
    let id = {
        let mut state = state.borrow_mut();
        push_tab(&mut state, url, title)
    };
    activate_tab(state, id);
}

fn reopen_closed_tab(state: &Rc<RefCell<AppState>>) {
    let closed = state.borrow_mut().closed.pop();
    let Some(closed) = closed else {
        return;
    };
    let url = canonicalize_internal(if closed.url.is_empty() {
        NEWTAB_URL.to_owned()
    } else {
        closed.url
    });
    let id = {
        let mut state = state.borrow_mut();
        push_tab(&mut state, url, closed.title)
    };
    activate_tab(state, id);
}

fn next_tab_id(state: &Rc<RefCell<AppState>>, delta: isize) -> Option<TabId> {
    let state = state.borrow();
    let index = state.tab_index(state.active)?;
    if state.tabs.len() < 2 {
        return None;
    }
    let next = (index as isize + delta).rem_euclid(state.tabs.len() as isize) as usize;
    Some(state.tabs[next].id)
}

fn tab_number_id(state: &Rc<RefCell<AppState>>, number: u32) -> Option<TabId> {
    let state = state.borrow();
    let length = state.tabs.len();
    if length == 0 {
        return None;
    }
    let index = if number == 9 {
        length - 1
    } else {
        number as usize - 1
    };
    state.tabs.get(index).map(|tab| tab.id)
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetKeyState(vkey: i32) -> i16;
}

#[cfg(target_os = "windows")]
fn handle_shortcut(
    state: &Rc<RefCell<AppState>>,
    virtual_key: u32,
    scan_code: u32,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    if ctrl && shift && (virtual_key == 0x4E || scan_code == 0x31) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = ProcessCommand::new(exe).arg("--private").spawn();
        }
        true
    } else if ctrl && shift && (virtual_key == 0x54 || scan_code == 0x14) {
        let _ = state.borrow().proxy.send_event(AppEvent::Reopen);
        true
    } else if ctrl && (virtual_key == 0x54 || scan_code == 0x14) {
        let _ = state.borrow().proxy.send_event(AppEvent::NewTab);
        true
    } else if ctrl && (virtual_key == 0x57 || scan_code == 0x11) {
        let active = state.borrow().active;
        let _ = state.borrow().proxy.send_event(AppEvent::Close(active));
        true
    } else if ctrl && virtual_key == 0x09 {
        if let Some(id) = next_tab_id(state, if shift { -1 } else { 1 }) {
            let _ = state.borrow().proxy.send_event(AppEvent::Activate(id));
        }
        true
    } else if ctrl && (0x31..=0x39).contains(&virtual_key) {
        if let Some(id) = tab_number_id(state, virtual_key - 0x30) {
            let _ = state.borrow().proxy.send_event(AppEvent::Activate(id));
        }
        true
    } else if ctrl && shift && virtual_key == 0x41 {
        {
            let mut state = state.borrow_mut();
            state.switcher_open = true;
        }
        apply_layout(state);
        push_ui_state(state);
        true
    } else if ctrl && shift && virtual_key == 0x47 {
        toggle_all_groups(state);
        true
    } else if ctrl && alt && virtual_key == 0x28 {
        cycle_group(state, 1);
        true
    } else if ctrl && alt && virtual_key == 0x26 {
        cycle_group(state, -1);
        true
    } else if ctrl && (virtual_key == 0x46 || scan_code == 0x21) {
        open_find(state);
        true
    } else if virtual_key == 0x1B && state.borrow().find_open {
        close_find(state);
        true
    } else if ctrl && (virtual_key == 0x4C || scan_code == 0x26) {
        focus_address(state);
        true
    } else if ctrl && (virtual_key == 0xBB || scan_code == 0x0D || virtual_key == 0x6B) {
        zoom_active(state, 0.1);
        true
    } else if ctrl && (virtual_key == 0xBD || scan_code == 0x0C || virtual_key == 0x6D) {
        zoom_active(state, -0.1);
        true
    } else if ctrl && virtual_key == 0x30 {
        apply_zoom(state, 1.0);
        true
    } else if virtual_key == 0x78 {
        let enabled = !state.borrow().vertical;
        let _ = state.borrow().proxy.send_event(AppEvent::Vertical(enabled));
        true
    } else if virtual_key == 0x7A {
        let window = state.borrow().window.clone();
        if window.fullscreen().is_some() {
            window.set_fullscreen(None);
        } else {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        true
    } else if (ctrl && (virtual_key == 0x52 || scan_code == 0x13)) || virtual_key == 0x74 {
        reload_active(state);
        true
    } else if virtual_key == 0x7B {
        open_devtools_active(state);
        true
    } else if alt && virtual_key == 0x25 {
        go_back_active(state);
        true
    } else if alt && virtual_key == 0x27 {
        go_forward_active(state);
        true
    } else if virtual_key == 0x1B && state.borrow().active_tab().is_some_and(|tab| tab.loading) {
        stop_active(state);
        true
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
fn install_accelerators_for_view(state: &Rc<RefCell<AppState>>, webview: &WebView) {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        },
    };

    let controller = webview.controller();
    let state = Rc::clone(state);
    let mut token = 0i64;
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };

        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
        unsafe {
            let _ = args.KeyEventKind(&mut kind);
        }
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
            && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
        {
            return Ok(());
        }

        let mut virtual_key = 0u32;
        unsafe {
            let _ = args.VirtualKey(&mut virtual_key);
        }

        let mut physical = unsafe { std::mem::zeroed::<COREWEBVIEW2_PHYSICAL_KEY_STATUS>() };
        unsafe {
            let _ = args.PhysicalKeyStatus(&mut physical);
        }

        let ctrl = unsafe { GetKeyState(0x11) } < 0;
        let alt = unsafe { GetKeyState(0x12) } < 0;
        let shift = unsafe { GetKeyState(0x10) } < 0;

        if handle_shortcut(&state, virtual_key, physical.ScanCode, ctrl, alt, shift) {
            unsafe {
                let _ = args.SetHandled(true);
            }
        }

        Ok(())
    }));

    unsafe {
        if let Err(error) = controller.add_AcceleratorKeyPressed(&handler, &mut token) {
            eprintln!("[shortcut] failed to register accelerator handler: {error}");
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn install_accelerators_for_view(_state: &Rc<RefCell<AppState>>, _webview: &WebView) {}

#[cfg(target_os = "windows")]
fn downloads_dir() -> PathBuf {
    let dir = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn unique_download_path(url: &str) -> PathBuf {
    let name = Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "download".to_owned());
    let name = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let dir = downloads_dir();
    let mut candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    for index in 1..1000 {
        let suffix = if ext.is_empty() {
            format!("{stem} ({index})")
        } else {
            format!("{stem} ({index}).{ext}")
        };
        candidate = dir.join(suffix);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

fn downloads_payload(state: &AppState) -> Value {
    let items: Vec<Value> = state
        .downloads
        .iter()
        .map(|download| {
            json!({
                "id": download.id,
                "url": download.url,
                "path": download.path,
                "received": download.received,
                "total": download.total,
                "state": download.state,
                "paused": download.paused,
            })
        })
        .collect();
    Value::Array(items)
}

#[cfg(target_os = "windows")]
fn update_download(
    state: &Rc<RefCell<AppState>>,
    id: u64,
    received: i64,
    total: i64,
    status: &str,
) {
    let updated = {
        let mut state = state.borrow_mut();
        if let Some(download) = state
            .downloads
            .iter_mut()
            .find(|download| download.id == id)
        {
            download.received = received;
            if total > 0 {
                download.total = total;
            }
            download.state = status.to_owned();
            true
        } else {
            false
        }
    };
    if updated {
        if let Some(storage) = &state.borrow().storage {
            let _ = storage.send(StorageCommand::UpdateDownload {
                id: id as i64,
                received,
                total,
                state: status.to_owned(),
            });
        }
        let _ = state.borrow().proxy.send_event(AppEvent::DownloadsChanged);
    }
}

#[cfg(target_os = "windows")]
fn download_state_name(operation: &ICoreWebView2DownloadOperation) -> &'static str {
    let mut value = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
    unsafe {
        let _ = operation.State(&mut value);
    }
    if value == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        "completed"
    } else if value == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
        "interrupted"
    } else {
        "progress"
    }
}

#[cfg(target_os = "windows")]
fn install_download_operation_handlers(
    state: &Rc<RefCell<AppState>>,
    operation: &ICoreWebView2DownloadOperation,
    id: u64,
) {
    let progress_state = Rc::clone(state);
    let mut progress_token = 0;
    let progress = BytesReceivedChangedEventHandler::create(Box::new(move |operation, _| {
        if let Some(operation) = operation {
            let mut received = 0;
            let mut total = 0;
            unsafe {
                let _ = operation.BytesReceived(&mut received);
                let _ = operation.TotalBytesToReceive(&mut total);
            }
            update_download(
                &progress_state,
                id,
                received,
                total,
                download_state_name(&operation),
            );
        }
        Ok(())
    }));
    unsafe {
        let _ = operation.add_BytesReceivedChanged(&progress, &mut progress_token);
    }

    let state_changed = Rc::clone(state);
    let mut state_token = 0;
    let changed = StateChangedEventHandler::create(Box::new(move |operation, _| {
        if let Some(operation) = operation {
            let mut received = 0;
            let mut total = 0;
            unsafe {
                let _ = operation.BytesReceived(&mut received);
                let _ = operation.TotalBytesToReceive(&mut total);
            }
            update_download(
                &state_changed,
                id,
                received,
                total,
                download_state_name(&operation),
            );
        }
        Ok(())
    }));
    unsafe {
        let _ = operation.add_StateChanged(&changed, &mut state_token);
    }
}

#[cfg(target_os = "windows")]
fn install_download_handler(state: &Rc<RefCell<AppState>>, webview: &WebView, _tab_id: TabId) {
    let state = Rc::clone(state);
    let mut token = 0;
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let Ok(operation) = (unsafe { args.DownloadOperation() }) else {
            return Ok(());
        };
        let url = {
            let mut raw = windows_core::PWSTR::null();
            unsafe {
                let _ = operation.Uri(&mut raw);
            }
            unsafe { raw.to_string().unwrap_or_default() }
        };
        let path = unique_download_path(&url);
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let _ = args.SetResultFilePath(PCWSTR(wide.as_ptr()));
            let _ = args.SetHandled(true);
        }
        let id = {
            let mut state = state.borrow_mut();
            let id = state.next_download_id;
            state.next_download_id += 1;
            state.downloads.push(DownloadEntry {
                id,
                url: url.clone(),
                path: path.to_string_lossy().into_owned(),
                received: 0,
                total: 0,
                state: "progress".to_owned(),
                paused: false,
                operation: Some(operation.clone()),
            });
            id
        };
        if let Some(storage) = &state.borrow().storage {
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            let _ = storage.send(StorageCommand::SaveDownload(DownloadRecord {
                id: id as i64,
                url: url.clone(),
                path: path.to_string_lossy().into_owned(),
                received: 0,
                total: 0,
                state: "progress".to_owned(),
                created_at,
            }));
        }
        install_download_operation_handlers(&state, &operation, id);
        let _ = state.borrow().proxy.send_event(AppEvent::DownloadsChanged);
        Ok(())
    }));
    if let Ok(webview4) = webview.webview().cast::<ICoreWebView2_4>() {
        unsafe {
            let _ = webview4.add_DownloadStarting(&handler, &mut token);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn install_download_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView, _tab_id: TabId) {}

#[cfg(target_os = "windows")]
fn clear_browsing_data(state: &Rc<RefCell<AppState>>) {
    let webview = state
        .borrow()
        .chrome
        .as_ref()
        .map(|chrome| chrome.webview().clone());
    let Some(webview) = webview else {
        return;
    };
    let Ok(webview13) = webview.cast::<ICoreWebView2_13>() else {
        return;
    };
    let Ok(profile) = (unsafe { webview13.Profile() }) else {
        return;
    };
    let Ok(profile2) = profile.cast::<ICoreWebView2Profile2>() else {
        return;
    };
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(|_result| Ok(())));
    unsafe {
        let _ = profile2.ClearBrowsingDataAll(&handler);
    }
}

#[cfg(not(target_os = "windows"))]
fn clear_browsing_data(_state: &Rc<RefCell<AppState>>) {}

fn favicon_name(url: &str) -> String {
    let key = Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|host| host.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_else(|| url.to_owned());
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}.png")
}

fn favicon_cache_path(name: &str) -> PathBuf {
    rustle_storage::data_dir().join("favicons").join(name)
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map(u32::from).unwrap_or(0);
        let b2 = chunk.get(2).copied().map(u32::from).unwrap_or(0);
        let value = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((value >> 18) & 63) as usize] as char);
        out.push(TABLE[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn favicon_data(name: &str) -> Option<String> {
    let bytes = fs::read(favicon_cache_path(name)).ok()?;
    let mime = image_mime(&bytes);
    if mime == "application/octet-stream" {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

fn cached_favicon(url: &str) -> Option<String> {
    let name = favicon_name(url);
    let path = favicon_cache_path(&name);
    let metadata = fs::metadata(&path).ok()?;
    let stale = metadata
        .modified()
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(30 * 24 * 60 * 60));
    if stale {
        let _ = fs::remove_file(&path);
        return None;
    }
    Some(name)
}

#[cfg(target_os = "windows")]
fn image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\x00\x00\x01\x00") {
        "image/x-icon"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| byte == b'<')
    {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

fn attribute_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let position = lower.find(name)?;
    let after = &tag[position + name.len()..];
    let equal = after.find('=')?;
    let rest = after[equal + 1..].trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let value = &rest[1..];
        let end = value.find(quote)?;
        Some(value[..end].replace("&amp;", "&"))
    } else {
        Some(rest.split_whitespace().next()?.to_owned())
    }
}

fn icon_href_from_html(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(found) = lower[offset..].find("<link") {
        let start = offset + found;
        let rest = &lower[start..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &html[start..start + end];
        let low_tag = &lower[start..start + end];
        if (low_tag.contains("icon") || low_tag.contains("apple-touch")) && low_tag.contains("href")
        {
            if let Some(href) = attribute_value(tag, "href") {
                if !href.is_empty() {
                    return Some(href);
                }
            }
        }
        offset = start + end + 1;
    }
    None
}

fn download_image(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url).timeout(Duration::from_secs(7)).call().ok()?;
    if response.status() != 200 {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(512 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    (image_mime(&bytes) != "application/octet-stream").then_some(bytes)
}

fn download_site_favicon(page_url: &str) -> Option<Vec<u8>> {
    let parsed = Url::parse(page_url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let origin = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or_default()
    );
    let icon_url = format!("{origin}/favicon.ico");
    download_image(&icon_url).or_else(|| {
        let response = ureq::get(page_url)
            .timeout(Duration::from_secs(7))
            .call()
            .ok()?;
        if response.status() != 200 {
            return None;
        }
        let mut html = String::new();
        response
            .into_reader()
            .take(512 * 1024)
            .read_to_string(&mut html)
            .ok()?;
        let href = icon_href_from_html(&html)?;
        let icon = Url::parse(page_url).ok()?.join(&href).ok()?;
        download_image(icon.as_str())
    })
}

fn favicon_for_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let name = favicon_name(url);
    let path = favicon_cache_path(&name);
    if !path.exists() {
        let bytes = download_site_favicon(url)?;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&path, bytes).ok()?;
    }
    favicon_data(&name)
}

fn fetch_site_favicon(state: &Rc<RefCell<AppState>>, tab_id: TabId, page_url: &str) {
    let proxy = state.borrow().proxy.clone();
    let private = state.borrow().private;
    if private {
        return;
    }
    let Ok(parsed) = Url::parse(page_url) else {
        return;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return;
    }
    let name = favicon_name(page_url);
    let page_owned = page_url.to_owned();
    std::thread::spawn(move || {
        if let Some(bytes) = download_site_favicon(&page_owned) {
            let _ = proxy.send_event(AppEvent::FaviconFetched(tab_id, name, bytes));
        }
    });
}

fn request_tab_favicon(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    let (webview, url, private) = {
        let state = state.borrow();
        let Some(tab) = state.tab(tab_id) else {
            return;
        };
        let Some(webview) = &tab.webview else {
            return;
        };
        (webview.webview(), tab.url.clone(), state.private)
    };
    if private || !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let Ok(webview15) = webview.cast::<ICoreWebView2_15>() else {
        return;
    };
    let state = Rc::clone(state);
    let completed = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
        if result.is_ok() {
            if let Some(stream) = stream {
                let bytes = read_istream(&stream);
                if !bytes.is_empty() {
                    let url = state
                        .borrow()
                        .tab(tab_id)
                        .map(|tab| tab.url.clone())
                        .unwrap_or_default();
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let name = favicon_name(&url);
                        let path = favicon_cache_path(&name);
                        if let Some(parent) = path.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        if fs::write(&path, bytes).is_ok() {
                            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                                tab.favicon = Some(name);
                            }
                            push_ui_state(&state);
                        }
                    }
                }
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &completed);
    }
    std::mem::forget(completed);
}

#[cfg(not(target_os = "windows"))]
fn request_tab_favicon(_state: &Rc<RefCell<AppState>>, _tab_id: TabId) {}

#[cfg(target_os = "windows")]
fn read_istream(stream: &IStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let mut read = 0u32;
        let result = unsafe {
            stream.Read(
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                Some(&mut read),
            )
        };
        if result.is_err() || read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read as usize]);
    }
    data
}

#[cfg(target_os = "windows")]
fn install_favicon_handler(state: &Rc<RefCell<AppState>>, webview: &WebView, tab_id: TabId) {
    let Ok(webview15) = webview.webview().cast::<ICoreWebView2_15>() else {
        return;
    };
    let state = Rc::clone(state);
    let callback_view = webview15.clone();
    let mut token = 0;
    let handler = FaviconChangedEventHandler::create(Box::new(move |_sender, _args| {
        let webview = callback_view.clone();
        let state = Rc::clone(&state);
        let completed = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
            if result.is_ok() {
                if let Some(stream) = stream {
                    let bytes = read_istream(&stream);
                    if !bytes.is_empty() {
                        let url = state
                            .borrow()
                            .tab(tab_id)
                            .map(|tab| tab.url.clone())
                            .unwrap_or_default();
                        if url.starts_with("http://") || url.starts_with("https://") {
                            let name = favicon_name(&url);
                            let path = favicon_cache_path(&name);
                            if let Some(parent) = path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            if fs::write(&path, bytes).is_ok() {
                                if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                                    tab.favicon = Some(name);
                                }
                                push_ui_state(&state);
                            }
                        }
                    }
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = webview.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &completed);
        }
        std::mem::forget(completed);
        Ok(())
    }));
    unsafe {
        let _ = webview15.add_FaviconChanged(&handler, &mut token);
    }
    std::mem::forget(handler);
}

#[cfg(not(target_os = "windows"))]
fn install_favicon_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView, _tab_id: TabId) {}

fn respond_storage(state: &Rc<RefCell<AppState>>, request_id: u64, payload: Value) {
    let _ = state.borrow().proxy.send_event(AppEvent::StorageResponse {
        request_id,
        payload,
    });
}

fn decode_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn parse_bookmarks_html(html: &str) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
    let mut folder = String::new();
    let mut rest = html;
    loop {
        let lower = rest.to_ascii_lowercase();
        let next_folder = lower.find("<h3");
        let next_link = lower.find("<a ");
        let take_folder = match (next_folder, next_link) {
            (Some(f), Some(a)) => f < a,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_folder {
            let start = next_folder.unwrap();
            let after = &rest[start..];
            if let Some(close) = after.to_ascii_lowercase().find("</h3>") {
                let raw = &after[..close];
                folder = raw
                    .split_once('>')
                    .map(|(_, text)| decode_html(text.trim()))
                    .unwrap_or_default();
                rest = &after[close + 5..];
            } else {
                break;
            }
            continue;
        }
        let start = next_link.unwrap();
        let after = &rest[start..];
        let Some(href_pos) = after.to_ascii_lowercase().find("href=") else {
            rest = &after[5..];
            continue;
        };
        let value_part = &after[href_pos + 5..];
        let Some(quote) = value_part
            .chars()
            .next()
            .filter(|ch| *ch == '"' || *ch == '\'')
        else {
            rest = &after[5..];
            continue;
        };
        let value_part = &value_part[1..];
        let Some(end_quote) = value_part.find(quote) else {
            break;
        };
        let url = decode_html(&value_part[..end_quote]);
        let after_url = &value_part[end_quote + 1..];
        let Some(open_end) = after_url.find('>') else {
            break;
        };
        let after_open = &after_url[open_end + 1..];
        let Some(close) = after_open.to_ascii_lowercase().find("</a>") else {
            break;
        };
        let title = decode_html(&after_open[..close]);
        if url.starts_with("http://") || url.starts_with("https://") {
            entries.push((url, title, folder.clone()));
        }
        rest = &after_open[close + 4..];
    }
    entries
}

fn import_bookmarks_file(state: &Rc<RefCell<AppState>>, request_id: u64) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("HTML", &["html", "htm"])
        .set_title("Импорт закладок")
        .pick_file()
    else {
        respond_storage(state, request_id, json!({ "ok": true, "cancelled": true }));
        return;
    };
    match fs::read_to_string(&path) {
        Ok(html) => {
            let entries = parse_bookmarks_html(&html);
            if entries.is_empty() {
                respond_storage(state, request_id, json!({ "ok": true, "imported": 0 }));
            } else {
                storage_query(state, request_id, StorageQuery::ImportBookmarks { entries });
            }
        }
        Err(error) => respond_storage(
            state,
            request_id,
            json!({ "ok": false, "error": error.to_string() }),
        ),
    }
}

fn export_bookmarks_file(state: &Rc<RefCell<AppState>>, request_id: u64, items: &[Value]) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("HTML", &["html"])
        .set_file_name("rustle-bookmarks.html")
        .set_title("Экспорт закладок")
        .save_file()
    else {
        respond_storage(state, request_id, json!({ "ok": true, "cancelled": true }));
        return;
    };
    let mut html = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    let mut current_folder = String::new();
    for item in items {
        let folder = item.get("folder").and_then(Value::as_str).unwrap_or("");
        if folder != current_folder {
            if !current_folder.is_empty() {
                html.push_str("</DL><p>\n");
            }
            current_folder = folder.to_owned();
            if !current_folder.is_empty() {
                html.push_str(&format!("<DT><H3>{}</H3>\n<DL><p>\n", current_folder));
            }
        }
        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
        let title = item.get("title").and_then(Value::as_str).unwrap_or(url);
        html.push_str(&format!("<DT><A HREF=\"{url}\">{title}</A>\n"));
    }
    if !current_folder.is_empty() {
        html.push_str("</DL><p>\n");
    }
    html.push_str("</DL><p>\n");
    let result = fs::write(&path, html);
    respond_storage(
        state,
        request_id,
        match result {
            Ok(()) => json!({ "ok": true, "path": path.to_string_lossy() }),
            Err(error) => json!({ "ok": false, "error": error.to_string() }),
        },
    );
}

#[cfg(target_os = "windows")]
fn apply_zoom(state: &Rc<RefCell<AppState>>, factor: f64) {
    let factor = factor.clamp(0.5, 3.0);
    {
        let mut state_ref = state.borrow_mut();
        let active = state_ref.active;
        let Some(tab) = state_ref.tab_mut(active) else {
            return;
        };
        tab.zoom = factor;
        if let Some(webview) = &tab.webview {
            unsafe {
                let _ = webview.controller().SetZoomFactor(factor);
            }
        }
    }
    push_ui_state(state);
}

#[cfg(not(target_os = "windows"))]
fn apply_zoom(_state: &Rc<RefCell<AppState>>, _factor: f64) {}

#[cfg(target_os = "windows")]
fn zoom_active(state: &Rc<RefCell<AppState>>, delta: f64) {
    let factor = state
        .borrow()
        .active_tab()
        .map(|tab| tab.zoom + delta)
        .unwrap_or(1.0);
    apply_zoom(state, factor);
}

#[cfg(not(target_os = "windows"))]
fn zoom_active(_state: &Rc<RefCell<AppState>>, _delta: f64) {}

fn open_find(state: &Rc<RefCell<AppState>>) {
    state.borrow_mut().find_open = true;
    apply_layout(state);
    if let Some(chrome) = &state.borrow().chrome {
        let _ = chrome
            .evaluate_script("window.rustle && window.rustle.dispatch({type:\"find.open\"});");
    }
}

fn close_find(state: &Rc<RefCell<AppState>>) {
    state.borrow_mut().find_open = false;
    apply_layout(state);
}

fn find_in_page(state: &Rc<RefCell<AppState>>, text: &str, forward: bool) {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned());
    let script = format!(
        "window.find({text}, false, {}, true, false, false, false);",
        if forward { "false" } else { "true" }
    );
    let state = state.borrow();
    if let Some(webview) = state.active_tab().and_then(|tab| tab.webview.as_ref()) {
        let _ = webview.evaluate_script(&script);
    }
}

fn css_hide_script(css: &str) -> String {
    let css = serde_json::to_string(css).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        "(function(){{var css={css};try{{var sheet=new CSSStyleSheet();sheet.replaceSync(css);document.adoptedStyleSheets=document.adoptedStyleSheets.concat([sheet]);}}catch(e){{}}var id='rustle-adblock';var s=document.getElementById(id);if(!s){{s=document.createElement('style');s.id=id;(document.head||document.documentElement).appendChild(s);}}s.textContent+=css;}})();"
    )
}

fn hide_css(selectors: &[String]) -> String {
    selectors
        .iter()
        .filter(|selector| !selector.trim().is_empty())
        .map(|selector| format!("{selector}{{display:none !important;}}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_cosmetic_filters(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    if !state.borrow().settings.adblock {
        return;
    }
    let cosmetic = {
        let state = state.borrow();
        let Some(tab) = state.tab(tab_id) else {
            return;
        };
        if !(tab.url.starts_with("http://") || tab.url.starts_with("https://")) {
            return;
        }
        if adblock_exception(&state.settings.adblock_exceptions, &tab.url, &tab.url) {
            return;
        }
        let Some(engine) = &state.adblock_engine else {
            return;
        };
        adblock::cosmetic(engine, &tab.url)
    };
    {
        let mut state = state.borrow_mut();
        if let Some(tab) = state.tab_mut(tab_id) {
            tab.cosmetic_generic = !cosmetic.generichide;
            tab.cosmetic_exceptions = cosmetic.exceptions;
            tab.cosmetic_injected.clear();
        }
    }
    if !cosmetic.hide_selectors.is_empty() {
        let script = css_hide_script(&hide_css(&cosmetic.hide_selectors));
        if let Some(webview) = state
            .borrow()
            .tab(tab_id)
            .and_then(|tab| tab.webview.as_ref())
        {
            let _ = webview.evaluate_script(&script);
        }
    }
    if !cosmetic.injected_script.is_empty() {
        if let Some(webview) = state
            .borrow()
            .tab(tab_id)
            .and_then(|tab| tab.webview.as_ref())
        {
            let _ = webview.evaluate_script(&cosmetic.injected_script);
        }
    }
}

fn apply_generic_cosmetic_filters(
    state: &Rc<RefCell<AppState>>,
    tab_id: TabId,
    classes: &[String],
    ids: &[String],
) {
    let fresh = {
        let state = state.borrow();
        let Some(tab) = state.tab(tab_id) else {
            return;
        };
        if !tab.cosmetic_generic {
            return;
        }
        let Some(engine) = &state.adblock_engine else {
            return;
        };
        adblock::generic_selectors(engine, classes, ids, &tab.cosmetic_exceptions)
    };
    let new_selectors = {
        let mut state = state.borrow_mut();
        let Some(tab) = state.tab_mut(tab_id) else {
            return;
        };
        let mut new_selectors = Vec::new();
        for selector in fresh {
            if tab.cosmetic_injected.insert(selector.clone()) {
                new_selectors.push(selector);
            }
        }
        new_selectors
    };
    if !new_selectors.is_empty() {
        let script = css_hide_script(&hide_css(&new_selectors));
        if let Some(webview) = state
            .borrow()
            .tab(tab_id)
            .and_then(|tab| tab.webview.as_ref())
        {
            let _ = webview.evaluate_script(&script);
        }
    }
}

fn maybe_upgrade_https(url: String, enabled: bool) -> String {
    if !enabled {
        return url;
    }
    let Ok(mut parsed) = Url::parse(&url) else {
        return url;
    };
    if parsed.scheme() != "http" {
        return url;
    }
    let host = parsed.host_str().unwrap_or("");
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return url;
    }
    if parsed.set_scheme("https").is_ok() {
        parsed.to_string()
    } else {
        url
    }
}

fn shutdown(state: &Rc<RefCell<AppState>>) {
    let clear = state.borrow().settings.clear_on_exit && !state.borrow().private;
    if clear {
        clear_browsing_data(state);
        if let Ok(storage) = rustle_storage::Storage::open_default() {
            let _ = storage.clear_history();
        }
        let _ = fs::remove_file(rustle_storage::session_path());
    } else {
        save_session_state(state, true);
    }
}

#[cfg(target_os = "windows")]
fn permission_label(kind: i32) -> &'static str {
    if kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA.0 {
        "доступ к камере"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE.0 {
        "доступ к микрофону"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION.0 {
        "геолокацию"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS.0 {
        "уведомления"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ.0 {
        "чтение буфера обмена"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE.0 {
        "доступ к файлам"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY.0 {
        "автовоспроизведение"
    } else if kind == COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS.0 {
        "доступ к датчикам"
    } else {
        "разрешение"
    }
}

#[cfg(target_os = "windows")]
fn install_permission_handler(state: &Rc<RefCell<AppState>>, webview: &WebView) {
    let state = Rc::clone(state);
    let mut token = 0;
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
        let mut raw = windows_core::PWSTR::null();
        unsafe {
            let _ = args.PermissionKind(&mut kind);
            let _ = args.Uri(&mut raw);
        }
        let kind = kind.0;
        if kind == COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY.0 {
            unsafe {
                let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW);
            }
            return Ok(());
        }
        let origin = unsafe { raw.to_string().unwrap_or_default() };
        let key = format!("{origin}:{kind}");
        let allowed = if let Some(value) = state.borrow().permissions.get(&key) {
            *value
        } else {
            let label = permission_label(kind);
            let answer = rfd::MessageDialog::new()
                .set_title("Разрешение")
                .set_description(format!("{origin}\n\nРазрешить {label}?"))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            let allowed = answer == rfd::MessageDialogResult::Yes;
            state.borrow_mut().permissions.insert(key, allowed);
            allowed
        };
        unsafe {
            let _ = args.SetState(if allowed {
                COREWEBVIEW2_PERMISSION_STATE_ALLOW
            } else {
                COREWEBVIEW2_PERMISSION_STATE_DENY
            });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview
            .webview()
            .add_PermissionRequested(&handler, &mut token);
    }
}

#[cfg(target_os = "windows")]
fn install_crash_handler(state: &Rc<RefCell<AppState>>, webview: &WebView, tab_id: TabId) {
    let state = Rc::clone(state);
    let mut token = 0;
    let handler = ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(_args) = args {
            set_tab_error(
                &state,
                tab_id,
                ErrorInfo {
                    code: "render".to_owned(),
                    message: "Процесс страницы упал. Нажмите «Повторить».".to_owned(),
                },
            );
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.webview().add_ProcessFailed(&handler, &mut token);
    }
}

#[cfg(not(target_os = "windows"))]
fn install_permission_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
fn install_crash_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView, _tab_id: TabId) {}

#[cfg(target_os = "windows")]
fn install_zoom_handler(state: &Rc<RefCell<AppState>>, webview: &WebView, tab_id: TabId) {
    let state = Rc::clone(state);
    let mut token = 0;
    let handler = ZoomFactorChangedEventHandler::create(Box::new(move |controller, _| {
        if let Some(controller) = controller {
            let mut factor = 1.0;
            unsafe {
                let _ = controller.ZoomFactor(&mut factor);
            }
            if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                tab.zoom = factor;
            }
            push_ui_state(&state);
        }
        Ok(())
    }));
    unsafe {
        let _ = webview
            .controller()
            .add_ZoomFactorChanged(&handler, &mut token);
    }
}

#[cfg(not(target_os = "windows"))]
fn install_zoom_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView, _tab_id: TabId) {}

#[cfg(target_os = "windows")]
fn adblock_request_type(
    context: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
) -> &'static str {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
        "document"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE {
        "image"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET {
        "stylesheet"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT {
        "script"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        || context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
    {
        "xmlhttprequest"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT {
        "font"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA {
        "media"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET {
        "websocket"
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING {
        "ping"
    } else {
        "other"
    }
}

#[cfg(target_os = "windows")]
fn custom_adblock_match(rules: &[String], url: &str) -> Option<bool> {
    for rule in rules {
        let rule = rule.trim();
        if rule.is_empty() || rule.starts_with('!') || rule.contains("##") || rule.contains("#@#") {
            continue;
        }
        let (allow, rule) = if let Some(rule) = rule.strip_prefix("@@") {
            (true, rule)
        } else {
            (false, rule)
        };
        let needle = rule
            .trim_start_matches("||")
            .trim_end_matches('^')
            .trim_end_matches('$')
            .trim_matches('|')
            .trim_matches('/');
        if needle.is_empty() {
            continue;
        }
        if url.contains(needle) {
            return Some(!allow);
        }
    }
    None
}

fn adblock_exception(exceptions: &[String], url: &str, source_url: &str) -> bool {
    let matches = |value: &str| {
        Url::parse(value)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
            .is_some_and(|host| {
                exceptions.iter().any(|exception| {
                    let exception = exception.trim().trim_start_matches('.').to_lowercase();
                    !exception.is_empty()
                        && (host == exception || host.ends_with(&format!(".{exception}")))
                })
            })
    };
    matches(url) || matches(source_url)
}

#[cfg(target_os = "windows")]
fn install_adblock_handler(state: &Rc<RefCell<AppState>>, webview: &WebView, tab_id: TabId) {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT;

    let environment = webview.environment();
    let state = Rc::clone(state);
    let mut token = 0;
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        if !state.borrow().settings.adblock {
            return Ok(());
        }
        let Ok(request) = (unsafe { args.Request() }) else {
            return Ok(());
        };
        let mut raw = windows_core::PWSTR::null();
        unsafe {
            let _ = request.Uri(&mut raw);
        }
        let url = unsafe { raw.to_string().unwrap_or_default() };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(());
        }
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
        unsafe {
            let _ = args.ResourceContext(&mut context);
        }
        let request_type = adblock_request_type(context);
        let source_url = state
            .borrow()
            .tab(tab_id)
            .map(|tab| tab.url.clone())
            .unwrap_or_default();
        let exceptions = state.borrow().settings.adblock_exceptions.clone();
        if adblock_exception(&exceptions, &url, &source_url) {
            return Ok(());
        }
        let custom_rules = state.borrow().settings.adblock_custom_rules.clone();
        let custom = custom_adblock_match(&custom_rules, &url);
        if custom == Some(false) {
            return Ok(());
        }
        let custom_engine_blocked = {
            let state = state.borrow();
            state.adblock_custom_engine.as_ref().is_some_and(|engine| {
                matches!(
                    adblock::check(engine, &url, &source_url, request_type),
                    adblock::Match::Block
                )
            })
        };
        let outcome = if custom == Some(true) || custom_engine_blocked {
            adblock::Match::Block
        } else {
            let state = state.borrow();
            let Some(engine) = &state.adblock_engine else {
                return Ok(());
            };
            adblock::check(engine, &url, &source_url, request_type)
        };
        match outcome {
            adblock::Match::Allow => {}
            adblock::Match::Block => {
                if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                    tab.blocked += 1;
                }
                if let Ok(response) = unsafe {
                    environment.CreateWebResourceResponse(
                        None::<&IStream>,
                        403,
                        PCWSTR::null(),
                        PCWSTR::null(),
                    )
                } {
                    unsafe {
                        let _ = args.SetResponse(&response);
                    }
                }
            }
            adblock::Match::Rewrite(rewritten) => {
                let wide: Vec<u16> = rewritten.encode_utf16().chain(Some(0)).collect();
                unsafe {
                    let _ = request.SetUri(PCWSTR(wide.as_ptr()));
                }
            }
        }
        Ok(())
    }));
    unsafe {
        let wide: Vec<u16> = "*\0".encode_utf16().collect();
        let _ = webview.webview().AddWebResourceRequestedFilter(
            PCWSTR(wide.as_ptr()),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        );
        let _ = webview
            .webview()
            .add_WebResourceRequested(&handler, &mut token);
    }
}

#[cfg(not(target_os = "windows"))]
fn install_adblock_handler(_state: &Rc<RefCell<AppState>>, _webview: &WebView, _tab_id: TabId) {}

fn storage_query(state: &Rc<RefCell<AppState>>, request_id: u64, query: StorageQuery) {
    if let Some(storage) = &state.borrow().storage {
        let _ = storage.send(StorageCommand::Query { request_id, query });
    }
}

fn handle_ipc(state: &Rc<RefCell<AppState>>, body: &str) {
    if body.len() > MAX_IPC_BYTES {
        eprintln!("[ipc] message too large: {} bytes", body.len());
        return;
    }

    let Ok(message) = serde_json::from_str::<serde_json::Value>(body) else {
        eprintln!("[ipc] invalid json");
        return;
    };

    match message.get("type").and_then(|value| value.as_str()) {
        Some("chrome.ready") => {
            mark("chrome-ready");
            state.borrow_mut().ready = true;
            send_app_info(state);
            push_ui_state(state);

            let stress_cycles = state.borrow_mut().stress_cycles.take();
            if let Some(cycles) = stress_cycles {
                for _ in 0..cycles {
                    new_tab(state);
                    let active = state.borrow().active;
                    close_tab(state, active);
                }
                println!("[stress] {cycles} tab cycles ok");
                let _ = state.borrow().proxy.send_event(AppEvent::Exit);
                return;
            }

            let overlay = {
                let state = state.borrow();
                state
                    .active_tab()
                    .is_some_and(|tab| tab.error.is_some() || is_overlay_url(&tab.url))
            };
            if overlay {
                focus_active_content(state);
            }
        }
        Some("nav.go") => {
            if let Some(url) = message.get("url").and_then(|value| value.as_str()) {
                let _ = state
                    .borrow()
                    .proxy
                    .send_event(AppEvent::Navigate(url.to_owned()));
            }
        }
        Some("nav.back") => go_back_active(state),
        Some("nav.forward") => go_forward_active(state),
        Some("nav.reload") => reload_active(state),
        Some("nav.stop") => stop_active(state),
        Some("nav.retry") => reload_active(state),
        Some("tabs.unloadInactive") => unload_inactive_tabs(state),
        Some("tabs.new") => {
            let _ = state.borrow().proxy.send_event(AppEvent::NewTab);
        }
        Some("tabs.reopen") => {
            let _ = state.borrow().proxy.send_event(AppEvent::Reopen);
        }
        Some("tabs.close") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let _ = state.borrow().proxy.send_event(AppEvent::Close(TabId(id)));
            }
        }
        Some("tabs.activate") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let _ = state
                    .borrow()
                    .proxy
                    .send_event(AppEvent::Activate(TabId(id)));
            }
        }
        Some("tabs.pin") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let pinned = message
                    .get("pinned")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true);
                set_tab_pinned(state, TabId(id), pinned);
            }
        }
        Some("groups.rename") => {
            if let (Some(id), Some(name)) = (
                message.get("groupId").and_then(|value| value.as_u64()),
                message.get("name").and_then(|value| value.as_str()),
            ) {
                rename_group(state, id, name.to_owned());
            }
        }
        Some("groups.create") => {
            let name = message
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("Группа")
                .to_owned();
            let tab = message
                .get("tabId")
                .and_then(|value| value.as_u64())
                .map(TabId);
            let _ = state
                .borrow()
                .proxy
                .send_event(AppEvent::CreateGroup { name, tab });
        }
        Some("groups.toggle") => {
            if let Some(id) = message.get("groupId").and_then(|value| value.as_u64()) {
                let _ = state.borrow().proxy.send_event(AppEvent::ToggleGroup(id));
            }
        }
        Some("groups.remove") => {
            if let Some(id) = message.get("groupId").and_then(|value| value.as_u64()) {
                let _ = state.borrow().proxy.send_event(AppEvent::RemoveGroup(id));
            }
        }
        Some("groups.color") => {
            if let Some(group_id) = message.get("groupId").and_then(|value| value.as_u64()) {
                {
                    let mut state = state.borrow_mut();
                    if let Some(group) = state.groups.iter_mut().find(|group| group.id == group_id)
                    {
                        group.hue = (group.hue + 47) % 360;
                    }
                }
                push_ui_state(state);
                save_session_due(state);
            }
        }
        Some("groups.autogroup") => auto_group_by_domain(state),
        Some("groups.newTab") => {
            if let Some(id) = message.get("groupId").and_then(|value| value.as_u64()) {
                let _ = state.borrow().proxy.send_event(AppEvent::NewTabInGroup(id));
            }
        }
        Some("tabs.container") => {
            if let (Some(id), Some(container)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("container").and_then(|value| value.as_str()),
            ) {
                set_tab_container(state, TabId(id), container);
            }
        }
        Some("tabs.mute") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                toggle_tab_muted(state, TabId(id));
            }
        }
        Some("tabs.move") => {
            if let (Some(tab), Some(target)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("target").and_then(|value| value.as_u64()),
            ) {
                let _ = state.borrow().proxy.send_event(AppEvent::MoveTab {
                    tab: TabId(tab),
                    target: TabId(target),
                });
            }
        }
        Some("tabs.moveGroup") => {
            if let Some(tab) = message.get("id").and_then(|value| value.as_u64()) {
                let group = message.get("groupId").and_then(|value| value.as_u64());
                let _ = state.borrow().proxy.send_event(AppEvent::MoveTabGroup {
                    tab: TabId(tab),
                    group,
                });
            }
        }
        Some("history.query") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                let query = message
                    .get("query")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned();
                let limit = message
                    .get("limit")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(100) as u32;
                storage_query(state, request_id, StorageQuery::History { query, limit });
            }
        }
        Some("history.delete") => {
            if let (Some(request_id), Some(entry_id)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("entryId").and_then(|value| value.as_i64()),
            ) {
                storage_query(
                    state,
                    request_id,
                    StorageQuery::DeleteHistory { id: entry_id },
                );
            }
        }
        Some("history.clear") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                storage_query(state, request_id, StorageQuery::ClearHistory);
            }
        }
        Some("bookmarks.list") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                storage_query(state, request_id, StorageQuery::Bookmarks);
            }
        }
        Some("bookmarks.toggle") => {
            if let (Some(request_id), Some(url)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("url").and_then(|value| value.as_str()),
            ) {
                let title = message
                    .get("title")
                    .and_then(|value| value.as_str())
                    .unwrap_or(url)
                    .to_owned();
                storage_query(
                    state,
                    request_id,
                    StorageQuery::ToggleBookmark {
                        url: url.to_owned(),
                        title,
                    },
                );
            }
        }
        Some("bookmarks.move") => {
            if let (Some(request_id), Some(entry_id), Some(folder)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("entryId").and_then(|value| value.as_i64()),
                message.get("folder").and_then(|value| value.as_str()),
            ) {
                storage_query(
                    state,
                    request_id,
                    StorageQuery::MoveBookmark {
                        id: entry_id,
                        folder: folder.to_owned(),
                    },
                );
            }
        }
        Some("bookmarks.import") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                import_bookmarks_file(state, request_id);
            }
        }
        Some("bookmarks.export") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                let items = message
                    .get("entries")
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default();
                export_bookmarks_file(state, request_id, &items);
            }
        }
        Some("bookmarks.remove") => {
            if let (Some(request_id), Some(entry_id)) = (
                message.get("id").and_then(|value| value.as_u64()),
                message.get("entryId").and_then(|value| value.as_i64()),
            ) {
                storage_query(
                    state,
                    request_id,
                    StorageQuery::RemoveBookmark { id: entry_id },
                );
            }
        }
        Some("favicon.query") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                let urls: Vec<String> = message
                    .get("urls")
                    .and_then(|value| value.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .take(96)
                            .collect()
                    })
                    .unwrap_or_default();
                let private = state.borrow().private;
                let proxy = state.borrow().proxy.clone();
                if private || urls.is_empty() {
                    let _ = proxy.send_event(AppEvent::TileFavicons {
                        request_id,
                        items: Value::Object(serde_json::Map::new()),
                    });
                } else {
                    std::thread::spawn(move || {
                        let mut items = serde_json::Map::new();
                        for url in urls {
                            if let Some(data) = favicon_for_url(&url) {
                                items.insert(url, Value::String(data));
                            }
                        }
                        let _ = proxy.send_event(AppEvent::TileFavicons {
                            request_id,
                            items: Value::Object(items),
                        });
                    });
                }
            }
        }
        Some("suggest.query") => {
            if let Some(request_id) = message.get("id").and_then(|value| value.as_u64()) {
                let query = message
                    .get("query")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned();
                let limit = message
                    .get("limit")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(8) as u32;
                storage_query(state, request_id, StorageQuery::Suggest { query, limit });
            }
        }
        Some("ui.vertical") => {
            let enabled = message
                .get("enabled")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let _ = state.borrow().proxy.send_event(AppEvent::Vertical(enabled));
        }
        Some("ui.theme") => {
            let mut settings = state.borrow().settings.clone();
            settings.theme = if settings.theme == "light" {
                "dark".to_owned()
            } else {
                "light".to_owned()
            };
            if !state.borrow().private {
                let _ = rustle_storage::save_settings(&settings);
            }
            state.borrow_mut().settings = settings;
            push_ui_state(state);
            apply_layout(state);
        }
        Some("sidebar.collapse") => {
            {
                let mut state = state.borrow_mut();
                state.sidebar_collapsed = !state.sidebar_collapsed;
                state.settings.sidebar_collapsed = state.sidebar_collapsed;
                if !state.private {
                    let _ = rustle_storage::save_settings(&state.settings);
                }
            }
            apply_layout(state);
            push_ui_state(state);
        }
        Some("sidebar.ready") => push_ui_state(state),
        Some("window.min") => {
            let window = state.borrow().window.clone();
            window.set_minimized(true);
        }
        Some("window.max") => {
            let window = state.borrow().window.clone();
            if window.is_maximized() {
                window.set_maximized(false);
            } else {
                window.set_maximized(true);
            }
        }
        Some("window.close") => {
            let _ = state.borrow().proxy.send_event(AppEvent::Exit);
        }
        Some("window.drag") => {
            let window = state.borrow().window.clone();
            start_window_drag(&window);
        }
        Some("zoom.reset") => apply_zoom(state, 1.0),
        Some("zoom.wheel") => {
            let delta = message
                .get("delta")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let step = if delta < 0.0 { 0.1 } else { -0.1 };
            zoom_active(state, step);
        }
        Some("tabs.newUrl") => {
            if let Some(url) = message.get("url").and_then(|value| value.as_str()) {
                let _ = state
                    .borrow()
                    .proxy
                    .send_event(AppEvent::OpenUrl(url.to_owned()));
            }
        }
        Some("ui.find") => {
            let open = message
                .get("open")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if open {
                open_find(state);
            } else {
                close_find(state);
            }
        }
        Some("find.query") => {
            let text = message
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let forward = message
                .get("forward")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            find_in_page(state, text, forward);
        }
        Some("settings.export") => export_config(state),
        Some("sessions.refresh") => refresh_saved_sessions(state),
        Some("sessions.save") => {
            if let Some(name) = message.get("name").and_then(|value| value.as_str()) {
                let tags: Vec<String> = message
                    .get("tags")
                    .and_then(|value| value.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str())
                            .map(|value| value.trim().to_owned())
                            .filter(|value| !value.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                save_named_session(state, name, &tags);
            }
        }
        Some("sessions.load") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_str()) {
                load_named_session(state, id);
            }
        }
        Some("sessions.delete") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_str()) {
                delete_named_session(state, id);
            }
        }
        Some("session.saveAs") => save_session_as(state),
        Some("adblock.whitelist") => whitelist_current(state),
        Some("page.pdf") => print_active_pdf(state),
        Some("page.screenshot") => screenshot_window(state),
        Some("session.openAs") => open_session_as(state),
        Some("settings.import") => import_config(state),
        Some("window.resizeStart") => resize_window_ipc(state, &message, true, false),
        Some("window.resizeMove") => resize_window_ipc(state, &message, false, false),
        Some("window.resizeEnd") => resize_window_ipc(state, &message, false, true),
        Some("ui.switcher") => {
            let open = message
                .get("open")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            state.borrow_mut().switcher_open = open;
            apply_layout(state);
        }
        Some("ui.prompt") => {
            let open = message
                .get("open")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            state.borrow_mut().prompt_open = open;
            apply_layout(state);
        }
        Some("ui.menu") => {
            let origin = match message.get("origin").and_then(|value| value.as_str()) {
                Some("sidebar") => native_menu::Origin::Sidebar,
                _ => native_menu::Origin::Chrome,
            };
            let x = message
                .get("x")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let y = message
                .get("y")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let kind = match message.get("kind").and_then(|value| value.as_str()) {
                Some("tab") => message
                    .get("id")
                    .and_then(|value| value.as_u64())
                    .map(|id| native_menu::Kind::Tab(TabId(id))),
                Some("group") => message
                    .get("groupId")
                    .and_then(|value| value.as_u64())
                    .map(native_menu::Kind::Group),
                _ => Some(native_menu::Kind::App),
            };
            if let Some(kind) = kind {
                native_menu::show(state, kind, origin, x as i32, y as i32);
            }
        }
        Some("ui.dropdown") => {
            let open = message
                .get("open")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let height = message
                .get("height")
                .and_then(|value| value.as_u64())
                .unwrap_or(360) as u32;
            let changed = {
                let mut state = state.borrow_mut();
                let changed = state.dropdown_open != open || state.dropdown_height != height;
                state.dropdown_open = open;
                state.dropdown_height = height;
                changed
            };
            if changed {
                apply_layout(state);
            }
        }
        Some("adblock.update") => {
            let proxy = state.borrow().proxy.clone();
            adblock::spawn_loader(proxy, true, true);
        }
        Some("data.clear") => clear_browsing_data(state),
        Some("downloads.pause") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                if let Some(download) = state
                    .borrow_mut()
                    .downloads
                    .iter_mut()
                    .find(|entry| entry.id == id)
                {
                    #[cfg(target_os = "windows")]
                    if let Some(operation) = &download.operation {
                        unsafe {
                            if download.paused {
                                let _ = operation.Resume();
                            } else {
                                let _ = operation.Pause();
                            }
                        }
                        download.paused = !download.paused;
                    }
                }
                push_ui_state(state);
            }
        }
        Some("downloads.cancel") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let mut cancelled = false;
                let mut progress = (0, 0);
                {
                    let mut state = state.borrow_mut();
                    if let Some(download) = state.downloads.iter_mut().find(|d| d.id == id) {
                        #[cfg(target_os = "windows")]
                        if let Some(operation) = &download.operation {
                            unsafe {
                                let _ = operation.Cancel();
                            }
                        }
                        download.state = "cancelled".to_owned();
                        progress = (download.received, download.total);
                        cancelled = true;
                    }
                }
                if cancelled {
                    if let Some(storage) = &state.borrow().storage {
                        let _ = storage.send(StorageCommand::UpdateDownload {
                            id: id as i64,
                            received: progress.0,
                            total: progress.1,
                            state: "cancelled".to_owned(),
                        });
                    }
                }
                if cancelled {
                    push_ui_state(state);
                }
            }
        }
        Some("downloads.open") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let path = state
                    .borrow()
                    .downloads
                    .iter()
                    .find(|download| download.id == id)
                    .map(|download| download.path.clone());
                if let Some(path) = path {
                    if Path::new(&path).exists() {
                        let _ = ProcessCommand::new("explorer").arg(&path).spawn();
                    }
                }
            }
        }
        Some("downloads.reveal") => {
            if let Some(id) = message.get("id").and_then(|value| value.as_u64()) {
                let path = state
                    .borrow()
                    .downloads
                    .iter()
                    .find(|download| download.id == id)
                    .map(|download| download.path.clone());
                if let Some(path) = path {
                    if Path::new(&path).exists() {
                        let _ = ProcessCommand::new("explorer")
                            .arg(format!("/select,{path}"))
                            .spawn();
                    }
                }
            }
        }
        Some("settings.save") => {
            let mut settings = state.borrow().settings.clone();
            if let Some(value) = message.get("searchEngine").and_then(|value| value.as_str()) {
                if matches!(value, "ddg" | "google" | "yandex" | "bing") {
                    settings.search_engine = value.to_owned();
                }
            }
            if let Some(value) = message.get("theme").and_then(|value| value.as_str()) {
                if matches!(value, "system" | "light" | "dark") {
                    settings.theme = value.to_owned();
                }
            }
            if let Some(value) = message.get("language").and_then(|value| value.as_str()) {
                if matches!(value, "ru" | "en") {
                    settings.language = value.to_owned();
                }
            }
            if let Some(value) = message
                .get("particlesEnabled")
                .and_then(|value| value.as_bool())
            {
                settings.particles_enabled = value;
            }
            if let Some(values) = message.get("containers").and_then(|value| value.as_array()) {
                settings.containers = values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(|value| value.trim().to_lowercase())
                    .filter(|value| !value.is_empty())
                    .collect();
            }
            if let Some(value) = message
                .get("defaultContainer")
                .and_then(|value| value.as_str())
            {
                settings.default_container = if value.is_empty() || value == "none" {
                    None
                } else {
                    Some(value.to_owned())
                };
            }
            if let Some(value) = message
                .get("particleIntensity")
                .and_then(|value| value.as_u64())
            {
                settings.particle_intensity = (value as u32).clamp(40, 200);
            }
            if let Some(value) = message.get("startup").and_then(|value| value.as_str()) {
                if matches!(value, "restore" | "newtab") {
                    settings.startup = value.to_owned();
                }
            }
            if let Some(value) = message.get("httpsFirst").and_then(|value| value.as_bool()) {
                settings.https_first = value;
            }
            if let Some(value) = message.get("clearOnExit").and_then(|value| value.as_bool()) {
                settings.clear_on_exit = value;
            }
            if let Some(value) = message.get("adblock").and_then(|value| value.as_bool()) {
                settings.adblock = value;
            }
            if let Some(value) = message.get("sidebarWidth").and_then(|value| value.as_u64()) {
                settings.sidebar_width = (value as u32).clamp(72, SIDEBAR_WIDTH);
            }
            if let Some(values) = message
                .get("adblockCustomRules")
                .and_then(|value| value.as_array())
            {
                settings.adblock_custom_rules = values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .collect();
            }
            if let Some(minutes) = message
                .get("autoSuspendMinutes")
                .and_then(|value| value.as_u64())
            {
                settings.auto_suspend_minutes = minutes.min(240) as u32;
            }
            if let Some(values) = message
                .get("adblockExceptions")
                .and_then(|value| value.as_array())
            {
                settings.adblock_exceptions = values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(|value| value.trim().trim_start_matches('.').to_lowercase())
                    .filter(|value| !value.is_empty())
                    .collect();
            }
            if !state.borrow().private {
                let _ = rustle_storage::save_settings(&settings);
            }
            state.borrow_mut().settings = settings;
            let rules = state.borrow().settings.adblock_custom_rules.clone();
            state.borrow_mut().adblock_custom_engine = adblock::build_custom(&rules);
            apply_layout(state);
            push_ui_state(state);
        }
        Some(other) => eprintln!("[ipc] unknown message: {other}"),
        None => eprintln!("[ipc] missing type"),
    }
}

fn create_sidebar(state: &Rc<RefCell<AppState>>) -> wry::Result<()> {
    let (height, window, sidebar_width) = {
        let state = state.borrow();
        (
            state.height,
            state.window.clone(),
            state.settings.sidebar_width.clamp(72, SIDEBAR_WIDTH),
        )
    };
    let mut context = state
        .borrow_mut()
        .context
        .take()
        .expect("WebContext must exist while creating sidebar");
    let ipc_state = Rc::clone(state);
    let result = WebViewBuilder::new_with_web_context(&mut context)
        .with_bounds(rect(0, 0, sidebar_width, height))
        .with_browser_accelerator_keys(false)
        .with_additional_browser_args(webview_browser_args())
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_url(SIDEBAR_URL)
        .with_ipc_handler(move |request| handle_ipc(&ipc_state, request.body()))
        .build_as_child(window.as_ref());
    state.borrow_mut().context = Some(context);
    let sidebar = result?;
    #[cfg(target_os = "windows")]
    attach_resize_passthrough(&sidebar);
    state.borrow_mut().sidebar = Some(sidebar);
    {
        let state_ref = state.borrow();
        if let Some(sidebar) = &state_ref.sidebar {
            install_accelerators_for_view(state, sidebar);
            let _ = sidebar.set_visible(true);
            let _ = sidebar.set_bounds(rect(0, 0, sidebar_width, height));
        }
    }
    apply_layout(state);
    raise_chrome(state);
    Ok(())
}

fn create_chrome(state: &Rc<RefCell<AppState>>) -> wry::Result<()> {
    let (width, window) = {
        let state = state.borrow();
        (state.width, state.window.clone())
    };

    let mut context = state
        .borrow_mut()
        .context
        .take()
        .expect("WebContext must exist while creating chrome");

    let ipc_state = Rc::clone(state);
    let result = WebViewBuilder::new_with_web_context(&mut context)
        .with_bounds(rect(0, 0, width, CHROME_COLLAPSED))
        .with_browser_accelerator_keys(false)
        .with_additional_browser_args(webview_browser_args())
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_url(CHROME_URL)
        .with_ipc_handler(move |request| handle_ipc(&ipc_state, request.body()))
        .build_as_child(window.as_ref());

    state.borrow_mut().context = Some(context);
    let chrome = result?;
    #[cfg(target_os = "windows")]
    attach_resize_passthrough(&chrome);
    state.borrow_mut().chrome = Some(chrome);

    {
        let state_ref = state.borrow();
        if let Some(chrome) = &state_ref.chrome {
            install_accelerators_for_view(state, chrome);
        }
    }

    apply_layout(state);
    raise_chrome(state);
    Ok(())
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct LowMousePoint {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct LowMsg {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: LowMousePoint,
    private: u32,
}

#[cfg(target_os = "windows")]
static RESIZE_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static RESIZE_MODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_Y: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_L: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_T: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_R: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static RESIZE_START_B: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowsHookExW(
        id: i32,
        callback: Option<unsafe extern "system" fn(i32, usize, isize) -> isize>,
        module: isize,
        thread: u32,
    ) -> isize;
    fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut LowMsg, hwnd: isize, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const LowMsg) -> i32;
    fn DispatchMessageW(msg: *const LowMsg) -> isize;
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn resize_mouse_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    const WM_LBUTTONDOWN: usize = 0x0201;
    const HTLEFT: usize = 10;
    const HTRIGHT: usize = 11;
    const HTTOP: usize = 12;
    const HTTOPLEFT: usize = 13;
    const HTTOPRIGHT: usize = 14;
    const HTBOTTOM: usize = 15;
    const HTBOTTOMLEFT: usize = 16;
    const HTBOTTOMRIGHT: usize = 17;
    const BORDER: i32 = 10;

    if code >= 0 {
        let hwnd = RESIZE_HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            let mut point = ResizePoint { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut point);
            }
            let mode = RESIZE_MODE.load(std::sync::atomic::Ordering::Relaxed);
            if wparam == WM_LBUTTONDOWN {
                let mut rect = ResizeRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                unsafe {
                    GetWindowRect(hwnd, &mut rect);
                }
                let left = point.x < rect.left + BORDER;
                let right = point.x >= rect.right - BORDER;
                let top = point.y < rect.top + BORDER;
                let bottom = point.y >= rect.bottom - BORDER;
                let hit = match (left, right, top, bottom) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, _, _, _) => HTLEFT,
                    (_, true, _, _) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => 0,
                };
                if hit != 0 {
                    RESIZE_MODE.store(hit as i32, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_X.store(point.x, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_Y.store(point.y, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_L.store(rect.left, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_T.store(rect.top, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_R.store(rect.right, std::sync::atomic::Ordering::Relaxed);
                    RESIZE_START_B.store(rect.bottom, std::sync::atomic::Ordering::Relaxed);
                    return 1;
                }
            } else if wparam == 0x0200 && mode != 0 {
                let dx = point.x - RESIZE_START_X.load(std::sync::atomic::Ordering::Relaxed);
                let dy = point.y - RESIZE_START_Y.load(std::sync::atomic::Ordering::Relaxed);
                let mut left = RESIZE_START_L.load(std::sync::atomic::Ordering::Relaxed);
                let mut top = RESIZE_START_T.load(std::sync::atomic::Ordering::Relaxed);
                let mut right = RESIZE_START_R.load(std::sync::atomic::Ordering::Relaxed);
                let mut bottom = RESIZE_START_B.load(std::sync::atomic::Ordering::Relaxed);
                if matches!(mode, 10 | 13 | 16) {
                    left += dx;
                }
                if matches!(mode, 11 | 14 | 17) {
                    right += dx;
                }
                if matches!(mode, 12..=14) {
                    top += dy;
                }
                if matches!(mode, 15..=17) {
                    bottom += dy;
                }
                if right - left < 720 {
                    if matches!(mode, 10 | 13 | 16) {
                        left = right - 720;
                    } else {
                        right = left + 720;
                    }
                }
                if bottom - top < 480 {
                    if matches!(mode, 12..=14) {
                        top = bottom - 480;
                    } else {
                        bottom = top + 480;
                    }
                }
                unsafe {
                    SetWindowPos(
                        hwnd,
                        0,
                        left,
                        top,
                        right - left,
                        bottom - top,
                        0x0004 | 0x0010,
                    );
                }
                return 1;
            } else if wparam == 0x0202 && mode != 0 {
                RESIZE_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
static RESIZE_OVERLAYS: std::sync::Mutex<[isize; 8]> = std::sync::Mutex::new([0; 8]);

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        param: isize,
    ) -> isize;
    fn SetLayeredWindowAttributes(hwnd: isize, color: u32, alpha: u8, flags: u32) -> i32;
    fn SetCapture(hwnd: isize) -> isize;
    fn GetCapture() -> isize;
    fn GetParent(hwnd: isize) -> isize;
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn overlay_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
    id: usize,
    _data: usize,
) -> isize {
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONUP: u32 = 0x0202;
    if msg == WM_LBUTTONDOWN {
        let parent = unsafe { GetParent(hwnd) };
        let mut point = ResizePoint { x: 0, y: 0 };
        let mut rect = ResizeRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetCursorPos(&mut point);
            GetWindowRect(parent, &mut rect);
            SetCapture(hwnd);
        }
        let hit = id as i32;
        RESIZE_MODE.store(hit, std::sync::atomic::Ordering::Relaxed);
        RESIZE_HWND.store(parent, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_X.store(point.x, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_Y.store(point.y, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_L.store(rect.left, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_T.store(rect.top, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_R.store(rect.right, std::sync::atomic::Ordering::Relaxed);
        RESIZE_START_B.store(rect.bottom, std::sync::atomic::Ordering::Relaxed);
        return 1;
    }
    if msg == WM_MOUSEMOVE && unsafe { GetCapture() } == hwnd {
        let mode = RESIZE_MODE.load(std::sync::atomic::Ordering::Relaxed);
        if mode != 0 {
            let parent = RESIZE_HWND.load(std::sync::atomic::Ordering::Relaxed);
            let mut point = ResizePoint { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut point);
            }
            let dx = point.x - RESIZE_START_X.load(std::sync::atomic::Ordering::Relaxed);
            let dy = point.y - RESIZE_START_Y.load(std::sync::atomic::Ordering::Relaxed);
            let mut left = RESIZE_START_L.load(std::sync::atomic::Ordering::Relaxed);
            let mut top = RESIZE_START_T.load(std::sync::atomic::Ordering::Relaxed);
            let mut right = RESIZE_START_R.load(std::sync::atomic::Ordering::Relaxed);
            let mut bottom = RESIZE_START_B.load(std::sync::atomic::Ordering::Relaxed);
            if matches!(mode, 10 | 13 | 16) {
                left += dx;
            }
            if matches!(mode, 11 | 14 | 17) {
                right += dx;
            }
            if matches!(mode, 12..=14) {
                top += dy;
            }
            if matches!(mode, 15..=17) {
                bottom += dy;
            }
            if right - left < 720 {
                if matches!(mode, 10 | 13 | 16) {
                    left = right - 720;
                } else {
                    right = left + 720;
                }
            }
            if bottom - top < 480 {
                if matches!(mode, 12..=14) {
                    top = bottom - 480;
                } else {
                    bottom = top + 480;
                }
            }
            unsafe {
                SetWindowPos(
                    parent,
                    0,
                    left,
                    top,
                    right - left,
                    bottom - top,
                    0x0004 | 0x0010,
                );
            }
            return 1;
        }
    }
    if msg == WM_LBUTTONUP {
        RESIZE_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
        unsafe {
            ReleaseCapture();
        }
        return 1;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn layout_resize_overlays(parent: isize) {
    let mut rect = ResizeRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetClientRect(parent, &mut rect);
    }
    let w = rect.right.max(0);
    let h = rect.bottom.max(0);
    let b = 10;
    let specs = [
        (0, 0, w, b, 12),
        (0, h - b, w, b, 15),
        (0, 0, b, h, 10),
        (w - b, 0, b, h, 11),
        (0, 0, 20, 20, 13),
        (w - 20, 0, 20, 20, 14),
        (0, h - 20, 20, 20, 16),
        (w - 20, h - 20, 20, 20, 17),
    ];
    let overlays = RESIZE_OVERLAYS.lock().ok().map(|guard| *guard);
    let Some(overlays) = overlays else {
        return;
    };
    for (index, (x, y, width, height, _hit)) in specs.iter().enumerate() {
        let hwnd = overlays[index];
        if hwnd != 0 {
            unsafe {
                SetWindowPos(hwnd, 0, *x, *y, *width, *height, 0x0004 | 0x0010);
                SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn install_resize_overlays(parent: isize) {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "\0".encode_utf16().collect();
    let specs = [
        (0, 0, 100, 10, 12),
        (0, 0, 100, 10, 15),
        (0, 0, 10, 100, 10),
        (0, 0, 10, 100, 11),
        (0, 0, 20, 20, 13),
        (0, 0, 20, 20, 14),
        (0, 0, 20, 20, 16),
        (0, 0, 20, 20, 17),
    ];
    let mut overlays = [0isize; 8];
    for (index, (_, _, width, height, hit)) in specs.iter().enumerate() {
        let hwnd = unsafe {
            CreateWindowExW(
                0x00080000 | 0x08000000,
                class.as_ptr(),
                title.as_ptr(),
                0x40000000 | 0x10000000,
                0,
                0,
                *width,
                *height,
                parent,
                0,
                0,
                0,
            )
        };
        if hwnd != 0 {
            unsafe {
                SetLayeredWindowAttributes(hwnd, 0, 1, 2);
                SetWindowSubclass(hwnd, Some(overlay_proc), *hit as usize, 0);
            }
            overlays[index] = hwnd;
        }
    }
    if let Ok(mut guard) = RESIZE_OVERLAYS.lock() {
        *guard = overlays;
    }
    layout_resize_overlays(parent);
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn install_resize_mouse_hook(hwnd: isize) {
    RESIZE_HWND.store(hwnd, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(|| unsafe {
        let hook = SetWindowsHookExW(14, Some(resize_mouse_hook), 0, 0);
        if hook == 0 {
            return;
        }
        let mut msg = LowMsg {
            hwnd: 0,
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            point: LowMousePoint { x: 0, y: 0 },
            private: 0,
        };
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct NcCalcSizeParams {
    rgrc: [ResizeRect; 3],
    dst: isize,
    src: isize,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct ResizePoint {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct ResizeRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetCursorPos(point: *mut ResizePoint) -> i32;
    fn ScreenToClient(hwnd: isize, point: *mut ResizePoint) -> i32;
    fn GetClientRect(hwnd: isize, rect: *mut ResizeRect) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut ResizeRect) -> i32;
    fn GetAncestor(hwnd: isize, flags: u32) -> isize;
    fn EnumChildWindows(
        hwnd: isize,
        callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
        data: isize,
    ) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(
        hwnd: isize,
        callback: Option<
            unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize,
        >,
        id: usize,
        data: usize,
    ) -> i32;
    fn DefSubclassProc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn resize_subclass(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    const WM_NCHITTEST: u32 = 0x0084;
    const HTCLIENT: isize = 1;
    const HTLEFT: isize = 10;
    const HTRIGHT: isize = 11;
    const HTTOP: isize = 12;
    const HTTOPLEFT: isize = 13;
    const HTTOPRIGHT: isize = 14;
    const HTBOTTOM: isize = 15;
    const HTBOTTOMLEFT: isize = 16;
    const HTBOTTOMRIGHT: isize = 17;
    const BORDER: i32 = 10;
    const WM_NCCALCSIZE: u32 = 0x0083;

    if msg == WM_NCCALCSIZE {
        let border = 8i32;
        unsafe {
            if wparam != 0 {
                let params = &mut *(lparam as *mut NcCalcSizeParams);
                params.rgrc[0].left += border;
                // top edge stays in the client area: no extra line above the toolbar
                params.rgrc[0].right -= border;
                params.rgrc[0].bottom -= border;
            } else {
                let rect = &mut *(lparam as *mut ResizeRect);
                rect.left += border;
                rect.top += border;
                rect.right -= border;
                rect.bottom -= border;
            }
        }
        return 0;
    }

    if msg == 0x0005 {
        layout_resize_overlays(hwnd);
    }

    if msg == WM_NCHITTEST {
        let mut point = ResizePoint { x: 0, y: 0 };
        let mut rect = ResizeRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetCursorPos(&mut point);
            ScreenToClient(hwnd, &mut point);
            GetClientRect(hwnd, &mut rect);
        }
        let left = point.x < BORDER;
        let right = point.x >= rect.right - BORDER;
        let top = point.y < BORDER;
        let bottom = point.y >= rect.bottom - BORDER;
        let hit = match (left, right, top, bottom) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTLEFT,
            (_, true, _, _) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ => HTCLIENT,
        };
        if hit != HTCLIENT {
            return hit;
        }
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn child_hit_subclass(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    const WM_NCHITTEST: u32 = 0x0084;
    const HTTRANSPARENT: isize = -1;
    const GA_ROOT: u32 = 2;
    const BORDER: i32 = 10;

    if msg == WM_NCHITTEST {
        let mut point = ResizePoint { x: 0, y: 0 };
        let mut rect = ResizeRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetCursorPos(&mut point);
            let root = GetAncestor(hwnd, GA_ROOT);
            GetWindowRect(root, &mut rect);
        }
        if point.x < rect.left + BORDER
            || point.x >= rect.right - BORDER
            || point.y < rect.top + BORDER
            || point.y >= rect.bottom - BORDER
        {
            return HTTRANSPARENT;
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn attach_child_callback(hwnd: isize, _data: isize) -> i32 {
    unsafe {
        SetWindowSubclass(hwnd, Some(child_hit_subclass), 3, 0);
    }
    1
}

#[cfg(target_os = "windows")]
fn attach_resize_passthrough(webview: &WebView) {
    unsafe {
        let hwnd = webview.hwnd().0 as isize;
        SetWindowSubclass(hwnd, Some(child_hit_subclass), 2, 0);
        EnumChildWindows(hwnd, Some(attach_child_callback), 0);
    }
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
fn refresh_resize_passthrough(state: &Rc<RefCell<AppState>>) {
    let state_ref = state.borrow();
    if let Some(chrome) = &state_ref.chrome {
        attach_resize_passthrough(chrome);
    }
    if let Some(sidebar) = &state_ref.sidebar {
        attach_resize_passthrough(sidebar);
    }
    for tab in &state_ref.tabs {
        if let Some(webview) = &tab.webview {
            attach_resize_passthrough(webview);
        }
    }
}

fn enable_frameless_resize(window: &Window) {
    const GWL_STYLE: i32 = -16;
    const WS_THICKFRAME: isize = 0x0004_0000;
    const WS_MINIMIZEBOX: isize = 0x0002_0000;
    const WS_MAXIMIZEBOX: isize = 0x0001_0000;
    const WS_CAPTION: isize = 0x00C0_0000;
    const WS_SYSMENU: isize = 0x0008_0000;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_FRAMECHANGED: u32 = 0x0020;

    let hwnd = window.hwnd();
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            (style & !(WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)) | WS_THICKFRAME,
        );
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        SetWindowSubclass(hwnd, Some(resize_subclass), 1, 0);
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

fn cleanup_stale_private_dirs() {
    let root = std::env::temp_dir();
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("RustlePrivate-") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified
            .elapsed()
            .map(|age| age.as_secs() > 3600)
            .unwrap_or(false)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn schedule_private_cleanup(root: PathBuf) {
    let path = root.to_string_lossy().replace('"', "");
    let command = format!("ping 127.0.0.1 -n 3 > nul & rmdir /s /q \"{path}\"");
    let _ = ProcessCommand::new("cmd").args(["/C", &command]).spawn();
}

#[cfg(target_os = "windows")]
fn webview2_version() -> String {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let paths = [
        format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}"),
        format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}"),
    ];
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let root = RegKey::predef(hive);
        for path in &paths {
            if let Ok(key) = root.open_subkey(path) {
                if let Ok(version) = key.get_value::<String, _>("pv") {
                    if !version.trim().is_empty() {
                        return version;
                    }
                }
            }
        }
    }
    "не найдена".to_owned()
}

#[cfg(target_os = "windows")]
fn webview2_runtime_installed() -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let paths = [
        format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}"),
        format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}"),
    ];
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let root = RegKey::predef(hive);
        for path in &paths {
            if let Ok(key) = root.open_subkey(path) {
                if let Ok(version) = key.get_value::<String, _>("pv") {
                    if !version.trim().is_empty() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
fn webview2_runtime_installed() -> bool {
    true
}

fn main() -> wry::Result<()> {
    START.get_or_init(Instant::now);
    mark("process");

    if !webview2_runtime_installed() {
        rfd::MessageDialog::new()
            .set_title("Rustle")
            .set_description(
                "Для работы Rustle нужен Microsoft Edge WebView2 Runtime.\n\nУстановите его с https://developer.microsoft.com/microsoft-edge/webview2/",
            )
            .set_level(rfd::MessageLevel::Error)
            .show();
        return Ok(());
    }

    let private = std::env::args().any(|arg| arg == "--private");
    if private {
        cleanup_stale_private_dirs();
    }

    if !private && !single_instance::acquire() {
        let payload = single_instance::payload_from_args();
        let notified = single_instance::notify_existing(&payload);
        if !notified {
            rfd::MessageDialog::new()
                .set_title("Rustle")
                .set_description("Rustle уже запущен. Если окно не открывается, завершите старый процесс rustle.exe в Диспетчере задач и запустите снова.")
                .set_level(rfd::MessageLevel::Info)
                .show();
        }
        return Ok(());
    }

    let startup_urls = single_instance::startup_urls();
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let ipc = if private {
        None
    } else {
        let ipc_proxy = proxy.clone();
        single_instance::start(move |message| {
            for line in message.lines() {
                if line == "__focus__" {
                    let _ = ipc_proxy.send_event(AppEvent::Focus);
                } else {
                    let _ = ipc_proxy.send_event(AppEvent::OpenUrl(line.to_owned()));
                }
            }
        })
    };

    let window = Rc::new(
        WindowBuilder::new()
            .with_title("Rustle")
            .with_window_icon(Some(app_icon()))
            .with_inner_size(TaoLogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(TaoLogicalSize::new(720.0, 480.0))
            .with_decorations(false)
            .with_visible(false)
            .build(&event_loop)
            .expect("create window"),
    );
    window.set_undecorated_shadow(false);
    #[cfg(target_os = "windows")]
    enable_frameless_resize(&window);
    mark("window");

    let stress_cycles = std::env::args().find_map(|arg| {
        arg.strip_prefix("--stress-cycles=")
            .and_then(|value| value.parse::<u32>().ok())
    });

    let size = window.inner_size().to_logical::<u32>(window.scale_factor());
    let settings = rustle_storage::load_settings();
    if !private {
        if let (Some(width), Some(height)) = (settings.window_width, settings.window_height) {
            if width >= 720 && height >= 480 {
                window.set_inner_size(TaoLogicalSize::new(width as f64, height as f64));
            }
        }
        if let (Some(x), Some(y)) = (settings.window_x, settings.window_y) {
            window.set_outer_position(tao::dpi::LogicalPosition::new(x, y));
        }
    }
    let private_root = private.then(|| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("RustlePrivate-{}-{stamp}", std::process::id()))
    });
    let webview_dir = private_root
        .as_ref()
        .map(|root| {
            let dir = root.join("WebView2");
            let _ = fs::create_dir_all(&dir);
            dir
        })
        .unwrap_or_else(data_directory);
    let context = WebContext::new(Some(webview_dir));
    let state = Rc::new(RefCell::new(AppState::new(
        window.clone(),
        context,
        size.width,
        size.height,
        proxy.clone(),
    )));

    let startup_bookmarks = if private {
        Vec::new()
    } else {
        rustle_storage::Storage::open_default()
            .ok()
            .and_then(|storage| storage.bookmarks().ok())
            .unwrap_or_default()
    };

    {
        let mut state = state.borrow_mut();
        state.stress_cycles = stress_cycles;
        state.settings = settings.clone();
        state.webview2_version = webview2_version();
        state.adblock_custom_engine = adblock::build_custom(&state.settings.adblock_custom_rules);
        state.private = private;
        state.vertical = settings.vertical_tabs;
        state.sidebar_collapsed = settings.sidebar_collapsed;

        state.bookmarks = startup_bookmarks;
        state.storage = if private {
            None
        } else {
            storage_worker::spawn(proxy.clone())
        };
    }

    adblock::spawn_loader(proxy.clone(), false, !private);
    {
        let autosave_proxy = proxy.clone();
        std::thread::spawn(move || {
            let mut ticks = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                if autosave_proxy.send_event(AppEvent::Autosave).is_err() {
                    break;
                }
                ticks += 1;
                if ticks % 12 == 0 {
                    let _ = autosave_proxy.send_event(AppEvent::Maintenance);
                }
            }
        });
    }

    let session = if private || settings.startup == "newtab" {
        None
    } else {
        rustle_storage::load_session()
    };
    let recovered = session.as_ref().is_some_and(|value| !value.clean_exit);
    restore_session(&state, session);
    state.borrow_mut().recovered = recovered;

    let active = state.borrow().active;
    ensure_tab_webview(&state, active);
    mark("content");

    create_chrome(&state)?;
    if state.borrow().vertical {
        create_sidebar(&state)?;
    }
    mark("chrome");
    refresh_saved_sessions(&state);
    {
        let resize_proxy = proxy.clone();
        std::thread::spawn(move || {
            for _ in 0..6 {
                std::thread::sleep(Duration::from_secs(1));
                let _ = resize_proxy.send_event(AppEvent::RefreshResize);
            }
        });
    }

    save_session_state(&state, false);
    if state.borrow().ready {
        send_app_info(&state);
        push_ui_state(&state);
    }

    window.set_visible(true);
    window.set_focus();
    mark("visible");

    for url in startup_urls {
        let _ = proxy.send_event(AppEvent::OpenUrl(url));
    }
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            let _ = proxy.send_event(AppEvent::FetchFavicons);
        });
    }

    let private_cleanup = private_root.clone();

    event_loop.run(move |event, _, control_flow| {
        let _ = &ipc;
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    shutdown(&state);
                    if let Some(root) = &private_cleanup {
                        schedule_private_cleanup(root.clone());
                    }
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Moved(position) => {
                    if !private {
                        let mut state = state.borrow_mut();
                        state.settings.window_x = Some(position.x);
                        state.settings.window_y = Some(position.y);
                        state.settings_dirty = true;
                    }
                }
                WindowEvent::Resized(size) => {
                    let logical = size.to_logical::<u32>(window.scale_factor());
                    if logical.width > 0 && logical.height > 0 {
                        {
                            let mut state = state.borrow_mut();
                            state.width = logical.width;
                            state.height = logical.height;
                            if !state.private {
                                state.settings.window_width = Some(logical.width);
                                state.settings.window_height = Some(logical.height);
                                state.settings_dirty = true;
                            }
                        }
                        apply_layout(&state);
                    }
                }
                _ => {}
            },
            Event::UserEvent(AppEvent::OpenUrl(url)) => {
                open_external_url(&state, &url);
            }
            Event::UserEvent(AppEvent::LayoutTick) => {
                apply_layout(&state);
            }
            Event::UserEvent(AppEvent::RefreshFavicon(tab_id)) => {
                request_tab_favicon(&state, tab_id);
            }
            Event::UserEvent(AppEvent::FetchFavicons) => {
                let tabs: Vec<(TabId, String)> = state
                    .borrow()
                    .tabs
                    .iter()
                    .filter(|tab| tab.url.starts_with("http"))
                    .map(|tab| (tab.id, tab.url.clone()))
                    .collect();
                for (tab_id, url) in tabs {
                    fetch_site_favicon(&state, tab_id, &url);
                }
            }
            Event::UserEvent(AppEvent::FaviconFetched(tab_id, name, bytes)) => {
                let path = favicon_cache_path(&name);
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if fs::write(&path, bytes).is_ok() {
                    if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                        tab.favicon = Some(name);
                    }
                    push_ui_state(&state);
                }
            }
            Event::UserEvent(AppEvent::ContentIpc(tab_id, body)) => {
                if let Ok(message) = serde_json::from_str::<Value>(&body) {
                    match message.get("type").and_then(Value::as_str) {
                        Some("zoom.wheel") => {
                            let delta =
                                message.get("delta").and_then(Value::as_f64).unwrap_or(0.0);
                            if state.borrow().active == tab_id {
                                let step = if delta < 0.0 { 0.1 } else { -0.1 };
                                let factor = state
                                    .borrow()
                                    .tab(tab_id)
                                    .map(|tab| tab.zoom + step)
                                    .unwrap_or(1.0);
                                apply_zoom(&state, factor);
                            }
                        }
                        Some("adblock.scan") => {
                            let classes: Vec<String> = message
                                .get("classes")
                                .and_then(Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|item| item.as_str().map(str::to_owned))
                                        .take(2000)
                                        .collect()
                                })
                                .unwrap_or_default();
                            let ids: Vec<String> = message
                                .get("ids")
                                .and_then(Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|item| item.as_str().map(str::to_owned))
                                        .take(1000)
                                        .collect()
                                })
                                .unwrap_or_default();
                            if !classes.is_empty() || !ids.is_empty() {
                                apply_generic_cosmetic_filters(&state, tab_id, &classes, &ids);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::UserEvent(AppEvent::LoadTimeout(tab_id)) => {
                let changed = {
                    if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
                        if tab.loading {
                            tab.loading = false;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if changed {
                    push_ui_state(&state);
                }
            }
            Event::UserEvent(AppEvent::CreateGroup { name, tab }) => {
                create_group(&state, name, tab);
            }
            Event::UserEvent(AppEvent::ToggleGroup(group_id)) => {
                toggle_group(&state, group_id);
            }
            Event::UserEvent(AppEvent::RemoveGroup(group_id)) => {
                remove_group(&state, group_id);
            }
            Event::UserEvent(AppEvent::NewTabInGroup(group_id)) => {
                new_tab_in_group(&state, group_id);
            }
            Event::UserEvent(AppEvent::MoveTab { tab, target }) => {
                move_tab(&state, tab, target);
            }
            Event::UserEvent(AppEvent::MoveTabGroup { tab, group }) => {
                move_tab_to_group(&state, tab, group);
            }
            Event::UserEvent(AppEvent::Vertical(enabled)) => {
                state.borrow_mut().layout_anim = None;
                let (from_top, from_side) = {
                    let state_ref = state.borrow();
                    let top = if state_ref.vertical {
                        CHROME_VERTICAL
                    } else {
                        CHROME_COLLAPSED
                    };
                    let side = if state_ref.vertical {
                        if state_ref.sidebar_collapsed {
                            SIDEBAR_COLLAPSED
                        } else {
                            SIDEBAR_WIDTH
                        }
                    } else {
                        0
                    };
                    (top, side)
                };
                let to_top = if enabled { CHROME_VERTICAL } else { CHROME_COLLAPSED };
                let to_side = if enabled {
                    if state.borrow().sidebar_collapsed {
                        SIDEBAR_COLLAPSED
                    } else {
                        SIDEBAR_WIDTH
                    }
                } else {
                    0
                };
                {
                    let mut state_ref = state.borrow_mut();
                    state_ref.vertical = enabled;
                    state_ref.settings.vertical_tabs = enabled;
                    if !state_ref.private {
                        let _ = rustle_storage::save_settings(&state_ref.settings);
                    }
                }
                start_layout_animation(&state, from_top, to_top, from_side, to_side);
                if enabled && state.borrow().sidebar.is_none() {
                    if let Err(error) = create_sidebar(&state) {
                        eprintln!("[sidebar] create failed: {error}");
                    }
                }
                apply_layout(&state);
                push_ui_state(&state);
                if !enabled {
                    focus_active_content(&state);
                }
            }
            Event::UserEvent(AppEvent::Focus) => {
                {
                    let window = state.borrow().window.clone();
                    window.set_minimized(false);
                    window.set_visible(true);
                    #[cfg(target_os = "windows")]
                    unsafe {
                        ShowWindow(window.hwnd(), 9);
                        SetForegroundWindow(window.hwnd());
                    }
                    window.set_focus();
                }
                focus_active_content(&state);
            }
            Event::UserEvent(AppEvent::NewTab) => {
                new_tab(&state);
            }
            Event::UserEvent(AppEvent::Activate(id)) => {
                activate_tab(&state, id);
            }
            Event::UserEvent(AppEvent::Close(id)) => {
                close_tab(&state, id);
            }
            Event::UserEvent(AppEvent::Reopen) => {
                reopen_closed_tab(&state);
            }
            Event::UserEvent(AppEvent::Navigate(input)) => {
                navigate_active(&state, &input);
            }
            Event::UserEvent(AppEvent::Title(tab_id, title)) => {
                on_title(&state, tab_id, title);
            }
            Event::UserEvent(AppEvent::RefreshResize) => {
                #[cfg(target_os = "windows")]
                refresh_resize_passthrough(&state);
            }
            Event::UserEvent(AppEvent::Maintenance) => {
                if state.borrow().ready {
                    suspend_idle_tabs(&state);
                }
            }
            Event::UserEvent(AppEvent::Autosave) => {
                if state.borrow().ready {
                    save_session_state(&state, false);
                    if state.borrow().settings_dirty && !state.borrow().private {
                        let settings = state.borrow().settings.clone();
                        let _ = rustle_storage::save_settings(&settings);
                        state.borrow_mut().settings_dirty = false;
                    }
                }
            }
            Event::UserEvent(AppEvent::UpdateStatus(status)) => {
                state.borrow_mut().update_status = status;
                push_ui_state(&state);
            }
            Event::UserEvent(AppEvent::AdblockReady(event)) => {
                match event {
                    adblock::AdblockEvent::Status(info) => {
                        state.borrow_mut().adblock_info = info;
                    }
                    adblock::AdblockEvent::Ready { engine, info } => {
                        let mut state = state.borrow_mut();
                        state.adblock_engine = Some(*engine);
                        state.adblock_info = info;
                    }
                }
                let tab_ids: Vec<TabId> = state.borrow().tabs.iter().map(|tab| tab.id).collect();
                for tab_id in tab_ids {
                    apply_cosmetic_filters(&state, tab_id);
                    if let Some(webview) = state
                        .borrow()
                        .tab(tab_id)
                        .and_then(|tab| tab.webview.as_ref())
                    {
                        let _ = webview.evaluate_script(
                            "window.__rustleAdblockRescan && window.__rustleAdblockRescan();",
                        );
                    }
                }
                push_ui_state(&state);
            }
            Event::UserEvent(AppEvent::TileFavicons { request_id, items }) => {
                let payload = json!({ "ok": true, "items": items });
                let script = format!(
                    "window.rustle && window.rustle.dispatch({{\"type\":\"storage.response\",\"id\":{request_id},\"payload\":{payload}}});"
                );
                if let Some(chrome) = &state.borrow().chrome {
                    let _ = chrome.evaluate_script(&script);
                }
            }
            Event::UserEvent(AppEvent::StorageResponse { request_id, payload }) => {
                let script = format!(
                    "window.rustle && window.rustle.dispatch({{\"type\":\"storage.response\",\"id\":{request_id},\"payload\":{payload}}});"
                );
                let state_ref = state.borrow();
                if let Some(chrome) = &state_ref.chrome {
                    let _ = chrome.evaluate_script(&script);
                }
            }
            Event::UserEvent(AppEvent::DownloadsChanged) => {
                push_ui_state(&state);
            }
            Event::UserEvent(AppEvent::BookmarksRestored(items)) => {
                {
                    let mut state = state.borrow_mut();
                    state.bookmarks = items;
                }
                apply_layout(&state);
                push_ui_state(&state);
            }
            Event::UserEvent(AppEvent::DownloadsRestored(records)) => {
                {
                let mut state = state.borrow_mut();
                state.downloads = records
                    .into_iter()
                    .map(|record| DownloadEntry {
                        id: record.id as u64,
                        url: record.url,
                        path: record.path,
                        received: record.received,
                        total: record.total,
                        state: record.state,
                        paused: false,
                        #[cfg(target_os = "windows")]
                        operation: None,
                    })
                    .collect();
                state.next_download_id = state
                    .downloads
                    .iter()
                    .map(|download| download.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                }
                push_ui_state(&state);
            }
            Event::UserEvent(AppEvent::Exit) => {
                shutdown(&state);
                if let Some(root) = &private_cleanup {
                    schedule_private_cleanup(root.clone());
                }
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod ipc_smoke_tests {
    use super::*;

    #[test]
    fn custom_rules_block_and_allow() {
        let rules = vec![
            "||ads.example.com^".to_owned(),
            "@@||good.example.com^".to_owned(),
        ];
        assert_eq!(
            custom_adblock_match(&rules, "https://ads.example.com/a.js"),
            Some(true)
        );
        assert_eq!(
            custom_adblock_match(&rules, "https://good.example.com/a.js"),
            Some(false)
        );
        assert_eq!(
            custom_adblock_match(&rules, "https://example.com/app.js"),
            None
        );
    }
}
