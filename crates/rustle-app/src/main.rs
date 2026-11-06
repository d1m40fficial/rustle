//! Rustle M0 skeleton: native window, HTML chrome WebView, content WebView,
//! custom protocol and a typed IPC channel.

use std::{borrow::Cow, cell::RefCell, path::PathBuf, rc::Rc, sync::OnceLock, time::Instant};

use rustle_core::VERSION;
use rustle_ui::{CSP, asset, mime_type};
use serde_json::json;
use tao::{
    dpi::LogicalSize as TaoLogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use url::Url;
use wry::{
    Rect, WebContext, WebView, WebViewBuilder, WebViewId,
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

struct AppState {
    content: Option<WebView>,
    chrome: Option<WebView>,
    width: u32,
    height: u32,
    chrome_height: u32,
    ready: bool,
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

fn normalize_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    if let Ok(url) = Url::parse(input) {
        if matches!(url.scheme(), "http" | "https" | "file") {
            return Some(url.to_string());
        }
    }

    Url::parse(&format!("https://{input}"))
        .ok()
        .map(|url| url.to_string())
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

fn navigate(state: &Rc<RefCell<AppState>>, input: &str) {
    let Some(url) = normalize_url(input) else {
        eprintln!("[nav] invalid address: {input}");
        return;
    };

    let state = state.borrow();
    if let Some(content) = &state.content {
        if let Err(error) = content.load_url(&url) {
            eprintln!("[nav] load_url failed: {error}");
        } else {
            println!("[nav] {url}");
        }
    }
}

fn eval_content(state: &Rc<RefCell<AppState>>, script: &str) {
    let state = state.borrow();
    if let Some(content) = &state.content {
        let _ = content.evaluate_script(script);
    }
}

fn toggle_overlay(state: &Rc<RefCell<AppState>>) {
    let mut state = state.borrow_mut();
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
        }
        Some("ui.toggleOverlay") => toggle_overlay(state),
        Some("nav.go") => {
            if let Some(url) = message.get("url").and_then(|value| value.as_str()) {
                navigate(state, url);
            }
        }
        Some("nav.back") => eval_content(state, "history.back()"),
        Some("nav.forward") => eval_content(state, "history.forward()"),
        Some("nav.reload") => eval_content(state, "location.reload()"),
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
            COREWEBVIEW2_PHYSICAL_KEY_STATUS,
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
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
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
            let is_l_key = virtual_key == 0x4C || (ctrl_down && physical.ScanCode == 0x26);

            if ctrl_down && is_l_key {
                println!("[shortcut] Ctrl+L");
                let state = state.borrow();
                if let Some(chrome) = &state.chrome {
                    let _ = chrome.focus();
                    let _ = chrome.evaluate_script(
                        "document.getElementById('address').focus(); \
                         document.getElementById('address').select();",
                    );
                    unsafe {
                        let _ = args.SetHandled(true);
                    }
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

fn main() -> wry::Result<()> {
    START.get_or_init(Instant::now);
    mark("process");

    let event_loop = EventLoop::new();
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

    let content = WebViewBuilder::new_with_web_context(&mut context)
        .with_bounds(rect(
            0,
            CHROME_COLLAPSED,
            size.width,
            size.height.saturating_sub(CHROME_COLLAPSED),
        ))
        .with_browser_accelerator_keys(false)
        .with_custom_protocol("rustle".into(), protocol_handler)
        .with_url(NEWTAB_URL)
        .build_as_child(&window)?;
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
    install_accelerators(&state);

    if state.borrow().ready {
        send_app_info(&state);
    }

    window.set_visible(true);
    mark("visible");

    event_loop.run(move |event, _, control_flow| {
        let _ = &context;
        *control_flow = ControlFlow::Wait;

        if let Event::WindowEvent { event, .. } = event {
            match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(size) => {
                    let logical = size.to_logical::<u32>(window.scale_factor());
                    let mut state = state.borrow_mut();
                    state.width = logical.width;
                    state.height = logical.height;

                    let chrome_height = state.chrome_height;
                    if let Some(chrome) = &state.chrome {
                        let _ = chrome.set_bounds(rect(0, 0, logical.width, chrome_height));
                    }
                    if let Some(content) = &state.content {
                        let _ = content.set_bounds(rect(
                            0,
                            CHROME_COLLAPSED,
                            logical.width,
                            logical.height.saturating_sub(CHROME_COLLAPSED),
                        ));
                    }
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn keeps_absolute_https_url() {
        assert_eq!(
            normalize_url("https://example.com/path").as_deref(),
            Some("https://example.com/path")
        );
    }

    #[test]
    fn adds_https_to_bare_host() {
        assert_eq!(
            normalize_url("example.com").as_deref(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn adds_https_to_localhost_with_port() {
        assert_eq!(
            normalize_url("localhost:8080").as_deref(),
            Some("https://localhost:8080/")
        );
    }

    #[test]
    fn rejects_javascript_url() {
        assert_eq!(normalize_url("javascript:alert(1)"), None);
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(normalize_url("   "), None);
    }
}
