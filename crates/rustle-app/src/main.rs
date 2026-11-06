//! Rustle M2.1: tabs, session restore, new-tab overlay and single instance.

mod single_instance;
mod storage_worker;

use std::{
    borrow::Cow,
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    rc::Rc,
    sync::OnceLock,
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use rustle_core::{ErrorInfo, TabId, VERSION, display_url, resolve_input, search_url};
use rustle_storage::{DownloadRecord, SESSION_VERSION, Session, SessionTab, Settings};
use rustle_ui::{CSP, asset, mime_type};
use serde_json::{Value, json};
use storage_worker::{StorageCommand, StorageQuery};
use tao::{
    dpi::LogicalSize as TaoLogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Window, WindowBuilder},
};
use url::Url;
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewId,
    dpi::{LogicalPosition, LogicalSize},
    http::{Request, Response},
};

#[cfg(target_os = "windows")]
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};

#[cfg(target_os = "windows")]
use webview2_com::{
    BytesReceivedChangedEventHandler, ClearBrowsingDataCompletedHandler,
    DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, ICoreWebView2_4, ICoreWebView2_13,
        ICoreWebView2DownloadOperation, ICoreWebView2Profile2,
    },
    StateChangedEventHandler,
};
#[cfg(target_os = "windows")]
use windows_core::{Interface, PCWSTR};

const CHROME_COLLAPSED: u32 = 78;
const CHROME_URL: &str = "rustle://localhost/chrome/index.html";
const NEWTAB_URL: &str = "rustle://localhost/newtab/index.html";
const MAX_IPC_BYTES: usize = 64 * 1024;
const MAX_CLOSED_TABS: usize = 25;

static START: OnceLock<Instant> = OnceLock::new();

fn mark(label: &str) {
    let start = START.get_or_init(Instant::now);
    println!("[startup] {:>5}ms {label}", start.elapsed().as_millis());
}

#[derive(Debug)]
enum AppEvent {
    OpenUrl(String),
    StorageResponse { request_id: u64, payload: Value },
    DownloadsChanged,
    DownloadsRestored(Vec<DownloadRecord>),
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
    downloads: Vec<DownloadEntry>,
    next_download_id: u64,
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
            downloads: Vec::new(),
            next_download_id: 1,
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
    state.tabs.push(Tab::new(id, url, title));
    id
}

fn save_session_state(state: &Rc<RefCell<AppState>>, clean_exit: bool) {
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
                })
                .collect(),
            closed: state
                .closed
                .iter()
                .map(|tab| SessionTab {
                    url: session_url(&tab.url),
                    title: tab.title.clone(),
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
    if session.tabs.is_empty() {
        let id = push_tab(
            &mut state,
            NEWTAB_URL.to_owned(),
            "Новая вкладка".to_owned(),
        );
        state.active = id;
    } else {
        for tab in session.tabs {
            let url = canonicalize_internal(if tab.url.is_empty() {
                NEWTAB_URL.to_owned()
            } else {
                tab.url
            });
            push_tab(&mut state, url, tab.title);
        }
        let active_index = session.active.min(state.tabs.len().saturating_sub(1));
        state.active = state.tabs[active_index].id;
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
                    "loading": tab.loading,
                    "active": tab.id == state.active,
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
            "settings": {
                "searchEngine": state.settings.search_engine,
                "theme": state.settings.theme,
                "startup": state.settings.startup,
            },
            "downloads": downloads_payload(&state),
        });

        let window_title = if title == "Новая вкладка" || title == "Rustle" {
            "Rustle".to_owned()
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

fn apply_layout(state: &Rc<RefCell<AppState>>) {
    let (width, height, active, overlay_mode, dropdown_open, dropdown_height) = {
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
        )
    };

    let chrome_height = if overlay_mode {
        height
    } else if dropdown_open {
        dropdown_height.clamp(CHROME_COLLAPSED, height)
    } else {
        CHROME_COLLAPSED
    };

    {
        let state = state.borrow();
        if let Some(chrome) = &state.chrome {
            let _ = chrome.set_bounds(rect(0, 0, width, chrome_height));
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
                    0,
                    CHROME_COLLAPSED,
                    width,
                    height.saturating_sub(CHROME_COLLAPSED),
                ));
            } else {
                let _ = webview.set_visible(false);
            }
        }
    }

    raise_chrome(state);
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
        tab.title.clear();
        tab.loading = true;
        had_error && active == tab_id
    };

    if collapse {
        apply_layout(state);
        focus_active_content(state);
    }
    push_ui_state(state);
    save_session_due(state);
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
        }
    }
    push_ui_state(state);
    save_session_due(state);
}

fn ensure_tab_webview(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    let (url, visible, width, height, window, proxy) = {
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
            state.window.clone(),
            state.proxy.clone(),
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
            CHROME_COLLAPSED,
            width,
            height.saturating_sub(CHROME_COLLAPSED),
        ))
        .with_visible(visible)
        .with_browser_accelerator_keys(false)
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
        .with_url(&url);

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
        tab.webview = Some(webview);
    }

    {
        let state_ref = state.borrow();
        if let Some(webview) = state_ref.tab(tab_id).and_then(|tab| tab.webview.as_ref()) {
            install_tab_navigation_handlers(state, webview, tab_id, proxy);
            install_download_handler(state, webview, tab_id);
            install_accelerators_for_view(state, webview);
        }
    }

    raise_chrome(state);
}

fn navigate_active(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(target) = resolve_input(input) else {
        return;
    };
    let engine = state.borrow().settings.search_engine.clone();
    let url = match target {
        rustle_core::NavigationTarget::Url(url) => canonicalize_internal(url),
        rustle_core::NavigationTarget::Search { query } => search_url(&engine, &query),
    };

    let active = state.borrow().active;
    let previous_url = {
        let state = state.borrow();
        let Some(tab) = state.tab(active) else {
            return;
        };
        tab.url.clone()
    };

    if is_overlay_url(&url) {
        {
            let mut state = state.borrow_mut();
            let Some(tab) = state.tab_mut(active) else {
                return;
            };
            if !is_overlay_url(&previous_url) && !previous_url.is_empty() {
                tab.return_url = Some(previous_url);
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
            tab.return_url.clone()
        } else {
            None
        }
    };
    if let Some(url) = overlay_return {
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

fn activate_tab(state: &Rc<RefCell<AppState>>, id: TabId) {
    let exists = state.borrow().tab(id).is_some();
    if !exists {
        return;
    }

    let previous = state.borrow().active;
    if previous == id {
        ensure_tab_webview(state, id);
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

    state.borrow_mut().active = id;
    ensure_tab_webview(state, id);
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

fn open_external_url(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(target) = resolve_input(input) else {
        return;
    };
    let url = match target {
        rustle_core::NavigationTarget::Url(url) => canonicalize_internal(url),
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
    if ctrl && shift && (virtual_key == 0x54 || scan_code == 0x14) {
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
    } else if ctrl && (virtual_key == 0x4C || scan_code == 0x26) {
        focus_address(state);
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
        Some("data.clear") => clear_browsing_data(state),
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
            if let Some(value) = message.get("startup").and_then(|value| value.as_str()) {
                if matches!(value, "restore" | "newtab") {
                    settings.startup = value.to_owned();
                }
            }
            let _ = rustle_storage::save_settings(&settings);
            state.borrow_mut().settings = settings;
            push_ui_state(state);
        }
        Some(other) => eprintln!("[ipc] unknown message: {other}"),
        None => eprintln!("[ipc] missing type"),
    }
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
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_url(CHROME_URL)
        .with_ipc_handler(move |request| handle_ipc(&ipc_state, request.body()))
        .build_as_child(window.as_ref());

    state.borrow_mut().context = Some(context);
    let chrome = result?;
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

fn main() -> wry::Result<()> {
    START.get_or_init(Instant::now);
    mark("process");

    if !single_instance::acquire() {
        let payload = single_instance::payload_from_args();
        let _ = single_instance::notify_existing(&payload);
        return Ok(());
    }

    let startup_urls = single_instance::startup_urls();
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let ipc_proxy = proxy.clone();
    let ipc = single_instance::start(move |message| {
        for line in message.lines() {
            if line == "__focus__" {
                let _ = ipc_proxy.send_event(AppEvent::Focus);
            } else {
                let _ = ipc_proxy.send_event(AppEvent::OpenUrl(line.to_owned()));
            }
        }
    });

    let window = Rc::new(
        WindowBuilder::new()
            .with_title("Rustle")
            .with_inner_size(TaoLogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(TaoLogicalSize::new(720.0, 480.0))
            .with_visible(false)
            .build(&event_loop)
            .expect("create window"),
    );
    mark("window");

    let stress_cycles = std::env::args().find_map(|arg| {
        arg.strip_prefix("--stress-cycles=")
            .and_then(|value| value.parse::<u32>().ok())
    });

    let size = window.inner_size().to_logical::<u32>(window.scale_factor());
    let settings = rustle_storage::load_settings();
    let context = WebContext::new(Some(data_directory()));
    let state = Rc::new(RefCell::new(AppState::new(
        window.clone(),
        context,
        size.width,
        size.height,
        proxy.clone(),
    )));

    {
        let mut state = state.borrow_mut();
        state.stress_cycles = stress_cycles;
        state.settings = settings.clone();
        state.storage = storage_worker::spawn(proxy.clone());
    }

    let session = if settings.startup == "newtab" {
        None
    } else {
        rustle_storage::load_session()
    };
    restore_session(&state, session);

    let active = state.borrow().active;
    ensure_tab_webview(&state, active);
    mark("content");

    create_chrome(&state)?;
    mark("chrome");

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

    event_loop.run(move |event, _, control_flow| {
        let _ = &ipc;
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    save_session_state(&state, true);
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(size) => {
                    let logical = size.to_logical::<u32>(window.scale_factor());
                    {
                        let mut state = state.borrow_mut();
                        state.width = logical.width;
                        state.height = logical.height;
                    }
                    apply_layout(&state);
                }
                _ => {}
            },
            Event::UserEvent(AppEvent::OpenUrl(url)) => {
                open_external_url(&state, &url);
            }
            Event::UserEvent(AppEvent::Focus) => {
                state.borrow().window.set_visible(true);
                state.borrow().window.set_focus();
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
                save_session_state(&state, true);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
