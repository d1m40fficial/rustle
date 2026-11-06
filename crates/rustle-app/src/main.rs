//! Rustle M1: navigation, address bar sync, history state, loading UI and
//! internal error view on top of the M0 WebView2 skeleton.

use std::{borrow::Cow, cell::RefCell, path::PathBuf, rc::Rc, sync::OnceLock, time::Instant};

use rustle_core::{ErrorInfo, VERSION, display_url, resolve_input};
use rustle_ui::{CSP, asset, mime_type};
use serde_json::json;
use tao::{
    dpi::LogicalSize as TaoLogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use url::Url;
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewId,
    dpi::{LogicalPosition, LogicalSize},
    http::{Request, Response},
};

#[cfg(target_os = "windows")]
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};

const CHROME_COLLAPSED: u32 = 78;
const CHROME_EXPANDED: u32 = 360;
const CHROME_URL: &str = "rustle://localhost/chrome/index.html";
const NEWTAB_URL: &str = "rustle://localhost/newtab/index.html";
const MAX_IPC_BYTES: usize = 64 * 1024;

static START: OnceLock<Instant> = OnceLock::new();

fn mark(label: &str) {
    let start = START.get_or_init(Instant::now);
    println!("[startup] {:>5}ms {label}", start.elapsed().as_millis());
}

#[derive(Debug)]
enum AppEvent {
    OpenUrl(String),
}

struct AppState {
    content: Option<WebView>,
    chrome: Option<WebView>,
    width: u32,
    height: u32,
    chrome_height: u32,
    ready: bool,
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    error: Option<ErrorInfo>,
}

impl AppState {
    fn new(width: u32, height: u32) -> Self {
        Self {
            content: None,
            chrome: None,
            width,
            height,
            chrome_height: CHROME_COLLAPSED,
            ready: false,
            url: NEWTAB_URL.to_owned(),
            title: "Новая вкладка".to_owned(),
            loading: false,
            can_back: false,
            can_forward: false,
            error: None,
        }
    }
}

fn data_directory() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let directory = base.join("Rustle").join("WebView2");
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
    if trimmed == "rustle://newtab" || url.starts_with("rustle://localhost/newtab") {
        NEWTAB_URL.to_owned()
    } else {
        url
    }
}

fn effective_title(state: &AppState) -> String {
    if state.error.is_some() {
        return "Не удалось открыть страницу".to_owned();
    }
    if !state.title.is_empty() {
        return state.title.clone();
    }
    if state.url.starts_with("rustle://") {
        return "Новая вкладка".to_owned();
    }
    Url::parse(&state.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Rustle".to_owned())
}

fn push_ui_state(state: &Rc<RefCell<AppState>>) {
    let payload = {
        let state = state.borrow();
        if !state.ready {
            return;
        }
        json!({
            "url": state.url,
            "displayUrl": display_url(&state.url),
            "title": effective_title(&state),
            "loading": state.loading,
            "canGoBack": state.can_back,
            "canGoForward": state.can_forward,
            "error": state.error.clone(),
        })
    };

    let script = format!(
        "window.rustle && window.rustle.dispatch({{\"type\":\"state\",\"payload\":{payload}}});"
    );
    let state = state.borrow();
    if let Some(chrome) = &state.chrome {
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

fn sync_from_content(state: &Rc<RefCell<AppState>>) {
    let snapshot = {
        let state = state.borrow();
        let Some(content) = &state.content else {
            return;
        };
        let url = content.url().unwrap_or_else(|_| state.url.clone());
        let (back, forward) = history_flags(content);
        (url, back, forward)
    };

    let mut state = state.borrow_mut();
    if state.error.is_none() {
        state.url = snapshot.0;
    }
    state.can_back = snapshot.1;
    state.can_forward = snapshot.2;
}

fn leave_error_mode(state: &Rc<RefCell<AppState>>) {
    let (width, height, had_error) = {
        let mut state = state.borrow_mut();
        if state.error.take().is_some() {
            state.chrome_height = CHROME_COLLAPSED;
            (state.width, state.height, true)
        } else {
            (0, 0, false)
        }
    };

    if !had_error {
        return;
    }

    let state = state.borrow();
    if let Some(chrome) = &state.chrome {
        let _ = chrome.set_bounds(rect(0, 0, width, CHROME_COLLAPSED));
    }
    if let Some(content) = &state.content {
        let _ = content.set_visible(true);
        let _ = content.set_bounds(rect(
            0,
            CHROME_COLLAPSED,
            width,
            height.saturating_sub(CHROME_COLLAPSED),
        ));
    }
}

fn navigate_to_url(state: &Rc<RefCell<AppState>>, url: String) {
    let url = canonicalize_internal(url);
    leave_error_mode(state);

    {
        let mut state = state.borrow_mut();
        state.url = url.clone();
        state.title.clear();
        state.loading = true;
        state.chrome_height = CHROME_COLLAPSED;

        let width = state.width;
        let height = state.height;
        if let Some(chrome) = &state.chrome {
            let _ = chrome.set_bounds(rect(0, 0, width, CHROME_COLLAPSED));
        }
        let load_result = if let Some(content) = &state.content {
            let _ = content.set_visible(true);
            let _ = content.set_bounds(rect(
                0,
                CHROME_COLLAPSED,
                width,
                height.saturating_sub(CHROME_COLLAPSED),
            ));
            let result = content.load_url(&url);
            let _ = content.focus();
            Some(result)
        } else {
            None
        };

        match load_result {
            Some(Ok(())) => println!("[nav] {url}"),
            Some(Err(error)) => {
                eprintln!("[nav] load_url failed: {error}");
                state.loading = false;
            }
            None => {
                state.loading = false;
            }
        }
    }

    push_ui_state(state);
}

fn navigate_input(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(target) = resolve_input(input) else {
        return;
    };
    navigate_to_url(state, target.into_url());
}

fn show_error(state: &Rc<RefCell<AppState>>, error: ErrorInfo) {
    let (width, height) = {
        let mut state = state.borrow_mut();
        state.error = Some(error);
        state.loading = false;
        state.title = "Не удалось открыть страницу".to_owned();
        state.chrome_height = state.height;
        (state.width, state.height)
    };

    let state_ref = state.borrow();
    if let Some(content) = &state_ref.content {
        let _ = content.set_visible(false);
    }
    if let Some(chrome) = &state_ref.chrome {
        let _ = chrome.set_bounds(rect(0, 0, width, height));
        let _ = chrome.focus();
    }
    drop(state_ref);

    push_ui_state(state);
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

fn reload(state: &Rc<RefCell<AppState>>) {
    if state.borrow().error.is_some() {
        let url = state.borrow().url.clone();
        navigate_to_url(state, url);
        return;
    }

    let loaded = {
        let state = state.borrow();
        state
            .content
            .as_ref()
            .map(|content| content.reload().is_ok())
            .unwrap_or(false)
    };
    if loaded {
        state.borrow_mut().loading = true;
        push_ui_state(state);
    }
}

fn stop(state: &Rc<RefCell<AppState>>) {
    let state_ref = state.borrow();
    if let Some(content) = &state_ref.content {
        let _ = content.evaluate_script("window.stop()");
    }
    drop(state_ref);

    state.borrow_mut().loading = false;
    push_ui_state(state);
}

fn go_back(state: &Rc<RefCell<AppState>>) {
    if !state.borrow().can_back {
        return;
    }
    leave_error_mode(state);

    let moved = {
        let state = state.borrow();
        state
            .content
            .as_ref()
            .map(|content| content.go_back().is_ok())
            .unwrap_or(false)
    };
    if moved {
        state.borrow_mut().loading = true;
        push_ui_state(state);
    }
}

fn go_forward(state: &Rc<RefCell<AppState>>) {
    if !state.borrow().can_forward {
        return;
    }
    leave_error_mode(state);

    let moved = {
        let state = state.borrow();
        state
            .content
            .as_ref()
            .map(|content| content.go_forward().is_ok())
            .unwrap_or(false)
    };
    if moved {
        state.borrow_mut().loading = true;
        push_ui_state(state);
    }
}

fn open_devtools(state: &Rc<RefCell<AppState>>) {
    let state = state.borrow();
    if let Some(content) = &state.content {
        content.open_devtools();
    }
}

fn toggle_overlay(state: &Rc<RefCell<AppState>>) {
    let mut state = state.borrow_mut();
    if state.error.is_some() {
        return;
    }

    let expanded = state.chrome_height == CHROME_COLLAPSED;
    let height = if expanded {
        CHROME_EXPANDED
    } else {
        CHROME_COLLAPSED
    };
    state.chrome_height = height;

    let width = state.width;
    if let Some(chrome) = &state.chrome {
        let _ = chrome.set_bounds(rect(0, 0, width, height));
        let script = format!("document.documentElement.classList.toggle('expanded', {expanded});");
        let _ = chrome.evaluate_script(&script);
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
        }
        Some("ui.toggleOverlay") => toggle_overlay(state),
        Some("nav.go") => {
            if let Some(url) = message.get("url").and_then(|value| value.as_str()) {
                navigate_input(state, url);
            }
        }
        Some("nav.back") => go_back(state),
        Some("nav.forward") => go_forward(state),
        Some("nav.reload") => reload(state),
        Some("nav.stop") => stop(state),
        Some("nav.retry") => reload(state),
        Some(other) => eprintln!("[ipc] unknown message: {other}"),
        None => eprintln!("[ipc] missing type"),
    }
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetKeyState(vkey: i32) -> i16;
}

#[cfg(target_os = "windows")]
fn install_accelerators(state: &Rc<RefCell<AppState>>) {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        },
    };

    let controllers = {
        let state = state.borrow();
        let mut controllers = Vec::new();
        if let Some(content) = &state.content {
            controllers.push(content.controller());
        }
        if let Some(chrome) = &state.chrome {
            controllers.push(chrome.controller());
        }
        controllers
    };

    for controller in controllers {
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

            let ctrl_down = unsafe { GetKeyState(0x11) } < 0;
            let alt_down = unsafe { GetKeyState(0x12) } < 0;
            let scan_code = physical.ScanCode;

            let handled = if ctrl_down && (virtual_key == 0x4C || scan_code == 0x26) {
                focus_address(&state);
                true
            } else if (ctrl_down && (virtual_key == 0x52 || scan_code == 0x13))
                || virtual_key == 0x74
            {
                reload(&state);
                true
            } else if virtual_key == 0x7B {
                open_devtools(&state);
                true
            } else if alt_down && virtual_key == 0x25 {
                go_back(&state);
                true
            } else if alt_down && virtual_key == 0x27 {
                go_forward(&state);
                true
            } else if virtual_key == 0x1B && state.borrow().loading {
                stop(&state);
                true
            } else {
                false
            };

            if handled {
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
}

#[cfg(target_os = "windows")]
fn install_navigation_handler(state: &Rc<RefCell<AppState>>) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
        },
        NavigationCompletedEventHandler,
    };
    use windows_core::BOOL;

    let webview = state
        .borrow()
        .content
        .as_ref()
        .map(|content| content.webview());
    let Some(webview) = webview else {
        return;
    };

    let state = Rc::clone(state);
    let mut token = 0i64;
    let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };

        let mut success = BOOL(0);
        unsafe {
            let _ = args.IsSuccess(&mut success);
        }

        if success.as_bool() {
            if state.borrow().error.is_some() {
                leave_error_mode(&state);
            }
            sync_from_content(&state);
            state.borrow_mut().loading = false;
            push_ui_state(&state);
            return Ok(());
        }

        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
        unsafe {
            let _ = args.WebErrorStatus(&mut status);
        }

        if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
            state.borrow_mut().loading = false;
            push_ui_state(&state);
            return Ok(());
        }

        let error = error_info(status);
        eprintln!("[nav] failed: {}", error.code);
        show_error(&state, error);
        Ok(())
    }));

    unsafe {
        if let Err(error) = webview.add_NavigationCompleted(&handler, &mut token) {
            eprintln!("[nav] failed to register completion handler: {error}");
        }
    }
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

fn main() -> wry::Result<()> {
    START.get_or_init(Instant::now);
    mark("process");

    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Rustle")
        .with_inner_size(TaoLogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(TaoLogicalSize::new(720.0, 480.0))
        .with_visible(false)
        .build(&event_loop)
        .expect("create window");
    mark("window");

    let size = window.inner_size().to_logical::<u32>(window.scale_factor());
    let state = Rc::new(RefCell::new(AppState::new(size.width, size.height)));

    let mut context = WebContext::new(Some(data_directory()));

    let content = {
        let navigation_state = Rc::clone(&state);
        let title_state = Rc::clone(&state);
        let load_state = Rc::clone(&state);

        WebViewBuilder::new_with_web_context(&mut context)
            .with_bounds(rect(
                0,
                CHROME_COLLAPSED,
                size.width,
                size.height.saturating_sub(CHROME_COLLAPSED),
            ))
            .with_browser_accelerator_keys(false)
            .with_custom_protocol("rustle".into(), protocol_handler)
            .with_navigation_handler(move |url| {
                if url.starts_with("rustle://localhost/chrome") {
                    return true;
                }
                {
                    let mut state = navigation_state.borrow_mut();
                    state.url = url;
                    state.title.clear();
                    state.loading = true;
                    state.error = None;
                }
                push_ui_state(&navigation_state);
                true
            })
            .with_document_title_changed_handler(move |title| {
                {
                    let mut state = title_state.borrow_mut();
                    if state.error.is_none() {
                        state.title = title;
                    }
                }
                push_ui_state(&title_state);
            })
            .with_on_page_load_handler(move |event, _url| {
                match event {
                    PageLoadEvent::Started => {
                        load_state.borrow_mut().loading = true;
                    }
                    PageLoadEvent::Finished => {
                        sync_from_content(&load_state);
                        load_state.borrow_mut().loading = false;
                    }
                }
                push_ui_state(&load_state);
            })
            .with_new_window_req_handler(move |url, _features| {
                let _ = proxy.send_event(AppEvent::OpenUrl(url));
                NewWindowResponse::Deny
            })
            .with_url(NEWTAB_URL)
            .build_as_child(&window)?
    };
    mark("content");
    state.borrow_mut().content = Some(content);

    let ipc_state = Rc::clone(&state);
    let chrome = WebViewBuilder::new_with_web_context(&mut context)
        .with_bounds(rect(0, 0, size.width, CHROME_COLLAPSED))
        .with_browser_accelerator_keys(false)
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_url(CHROME_URL)
        .with_ipc_handler(move |request| handle_ipc(&ipc_state, request.body()))
        .build_as_child(&window)?;
    mark("chrome");
    state.borrow_mut().chrome = Some(chrome);

    install_navigation_handler(&state);
    install_accelerators(&state);

    if state.borrow().ready {
        send_app_info(&state);
        push_ui_state(&state);
    }

    window.set_visible(true);
    mark("visible");

    event_loop.run(move |event, _, control_flow| {
        let _ = &context;
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(size) => {
                    let logical = size.to_logical::<u32>(window.scale_factor());
                    let mut state = state.borrow_mut();
                    state.width = logical.width;
                    state.height = logical.height;

                    let error_mode = state.error.is_some();
                    if error_mode {
                        state.chrome_height = state.height;
                    }

                    let chrome_height = state.chrome_height;
                    if let Some(chrome) = &state.chrome {
                        let _ = chrome.set_bounds(rect(0, 0, logical.width, chrome_height));
                    }
                    if let Some(content) = &state.content {
                        if error_mode {
                            let _ = content.set_visible(false);
                        } else {
                            let _ = content.set_visible(true);
                            let _ = content.set_bounds(rect(
                                0,
                                CHROME_COLLAPSED,
                                logical.width,
                                logical.height.saturating_sub(CHROME_COLLAPSED),
                            ));
                        }
                    }
                }
                _ => {}
            },
            Event::UserEvent(AppEvent::OpenUrl(url)) => {
                navigate_to_url(&state, url);
            }
            _ => {}
        }
    });
}
