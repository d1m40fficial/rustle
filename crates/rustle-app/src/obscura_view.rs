use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use rustle_obscura::ObscuraRenderer;
use rustle_obscura::cdp::ObscuraServer;

use crate::{AppState, push_ui_state};
use std::cell::RefCell;
use std::rc::Rc;

struct ViewerTab {
    id: u64,
    target: String,
    session: String,
    title: String,
    url: String,
    profile: u64,
}

struct Bridge {
    server: ObscuraServer,
    tabs: Vec<ViewerTab>,
    active: Option<u64>,
    next_id: u64,
    profile: u64,
    contexts: HashMap<u64, String>,
    status: String,
}

static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

pub fn handle(tab_id: crate::TabId, message: &Value, state: &Rc<RefCell<AppState>>) {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
    if !kind.starts_with("obscura.") {
        return;
    }
    if kind.starts_with("obscura.surface.") {
        native_handle(state, tab_id, message);
        return;
    }
    let mut guard = match BRIDGE.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if guard.is_none() {
        let names: Vec<(u64, String)> = state
            .borrow()
            .profiles
            .profiles
            .iter()
            .map(|profile| (profile.id, profile.name.clone()))
            .collect();
        match start_bridge(&names) {
            Ok(bridge) => *guard = Some(bridge),
            Err(error) => {
                drop(guard);
                send_error(state, tab_id, &error);
                return;
            }
        }
    }
    let Some(bridge) = guard.as_mut() else {
        return;
    };

    match kind {
        "obscura.start" => {}
        "obscura.newTab" => {
            let url = message
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("about:blank");
            if let Err(error) = bridge.new_tab(url) {
                bridge.status = error;
            }
        }
        "obscura.activate" => {
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                if bridge.tabs.iter().any(|tab| tab.id == id) {
                    bridge.active = Some(id);
                    let _ = bridge.ensure_screenshot();
                }
            }
        }
        "obscura.close" => {
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                bridge.close_tab(id);
            }
        }
        "obscura.navigate" => {
            let url = message.get("url").and_then(Value::as_str).unwrap_or("");
            if let Err(error) = bridge.navigate(url) {
                bridge.status = error;
            }
        }
        "obscura.click" => {
            let x = message.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = message.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            if let Err(error) = bridge.click(x, y) {
                bridge.status = error;
            }
        }
        "obscura.key" => {
            let key = message.get("key").and_then(Value::as_str).unwrap_or("");
            if let Err(error) = bridge.key(key) {
                bridge.status = error;
            }
        }
        "obscura.profile" => {
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                bridge.profile = id;
                if let Err(error) = bridge.ensure_context(id) {
                    bridge.status = error;
                }
                if let Err(error) = bridge.new_tab("about:blank") {
                    bridge.status = error;
                }
            }
        }
        "obscura.reload" => {
            if let Err(error) = bridge.reload() {
                bridge.status = error;
            }
        }
        _ => {}
    }

    let payload = bridge.payload(&state.borrow());
    drop(guard);
    send_update(state, tab_id, payload);
}

fn start_bridge(names: &[(u64, String)]) -> Result<Bridge, String> {
    let renderer = ObscuraRenderer::locate().map_err(|error| error.to_string())?;
    let server = ObscuraServer::start(renderer.binary()).map_err(|error| error.to_string())?;
    let mut bridge = Bridge {
        server,
        tabs: Vec::new(),
        active: None,
        next_id: 1,
        profile: names.first().map(|(id, _)| *id).unwrap_or(1),
        contexts: HashMap::new(),
        status: "Obscura CDP запущен".to_owned(),
    };
    bridge.ensure_context(bridge.profile)?;
    bridge.new_tab("about:blank")?;
    Ok(bridge)
}

impl Bridge {
    fn ensure_context(&mut self, profile: u64) -> Result<String, String> {
        if let Some(context) = self.contexts.get(&profile) {
            return Ok(context.clone());
        }
        let context = self
            .server
            .client
            .create_context()
            .map_err(|error| error.to_string())?;
        self.contexts.insert(profile, context.clone());
        Ok(context)
    }

    fn new_tab(&mut self, url: &str) -> Result<(), String> {
        let context = self.ensure_context(self.profile)?;
        let target = self
            .server
            .client
            .create_target("about:blank", Some(&context))
            .map_err(|error| error.to_string())?;
        let session = self
            .server
            .client
            .attach(&target)
            .map_err(|error| error.to_string())?;
        let _ = self
            .server
            .client
            .command("Page.enable", json!({}), Some(&session));
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(ViewerTab {
            id,
            target,
            session,
            title: "Новая вкладка".to_owned(),
            url: url.to_owned(),
            profile: self.profile,
        });
        self.active = Some(id);
        if url != "about:blank" {
            self.navigate(url)?;
        } else {
            self.status = "Вкладка Obscura создана".to_owned();
        }
        Ok(())
    }

    fn active_tab(&mut self) -> Option<&mut ViewerTab> {
        let id = self.active?;
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    fn navigate(&mut self, url: &str) -> Result<(), String> {
        if url.is_empty() {
            return Ok(());
        }
        let Some(tab) = self.active_tab() else {
            return Err("Нет активной вкладки".to_owned());
        };
        tab.url = url.to_owned();
        tab.title = url.to_owned();
        let session = tab.session.clone();
        self.server
            .client
            .command("Page.navigate", json!({"url": url}), Some(&session))
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_millis(900));
        self.status = format!("Открыто: {url}");
        Ok(())
    }

    fn reload(&mut self) -> Result<(), String> {
        let Some(tab) = self.active_tab() else {
            return Err("Нет активной вкладки".to_owned());
        };
        let url = tab.url.clone();
        let session = tab.session.clone();
        self.server
            .client
            .command("Page.navigate", json!({"url": url}), Some(&session))
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }

    fn click(&mut self, x: f64, y: f64) -> Result<(), String> {
        let Some(tab) = self.active_tab() else {
            return Err("Нет активной вкладки".to_owned());
        };
        let session = tab.session.clone();
        self.server
            .client
            .mouse(&session, x, y)
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_millis(300));
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), String> {
        let Some(tab) = self.active_tab() else {
            return Err("Нет активной вкладки".to_owned());
        };
        let session = tab.session.clone();
        for kind in ["keyDown", "keyUp"] {
            self.server
                .client
                .command(
                    "Input.dispatchKeyEvent",
                    json!({"type": kind, "key": key, "text": key}),
                    Some(&session),
                )
                .map_err(|error| error.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    fn close_tab(&mut self, id: u64) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
            let tab = self.tabs.remove(index);
            let _ = self.server.client.command(
                "Target.closeTarget",
                json!({"targetId": tab.target}),
                None,
            );
            if self.active == Some(id) {
                self.active = self.tabs.last().map(|tab| tab.id);
            }
        }
    }

    fn ensure_screenshot(&mut self) -> Result<(), String> {
        let Some(tab) = self.active_tab() else {
            return Ok(());
        };
        let session = tab.session.clone();
        self.server
            .client
            .screenshot(&session)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn payload(&mut self, state: &AppState) -> Value {
        let session = self.active_tab().map(|tab| tab.session.clone());
        let screenshot = session
            .and_then(|session| self.server.client.screenshot(&session).ok())
            .map(|png| {
                use base64::Engine;
                format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(png)
                )
            })
            .unwrap_or_default();
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .map(|tab| {
                json!({
                    "id": tab.id,
                    "title": tab.title,
                    "url": tab.url,
                    "active": Some(tab.id) == self.active,
                    "profile": tab.profile,
                })
            })
            .collect();
        let profiles: Vec<Value> = state
            .profiles
            .profiles
            .iter()
            .map(|profile| json!({"id": profile.id, "name": profile.name}))
            .collect();
        json!({
            "tabs": tabs,
            "profiles": profiles,
            "profile": self.profile,
            "active": self.active,
            "screenshot": screenshot,
            "status": self.status,
        })
    }
}

fn send_update(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, payload: Value) {
    let script = format!(
        "window.rustleObscura && window.rustleObscura.update({});",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned())
    );
    if let Some(webview) = state
        .borrow()
        .tab(tab_id)
        .and_then(|tab| tab.webview.as_ref())
    {
        let _ = webview.evaluate_script(&script);
    }
}

fn send_error(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, error: &str) {
    let payload =
        json!({"error": error, "tabs": [], "profiles": [], "screenshot": "", "status": error});
    send_update(state, tab_id, payload);
}

// Native-tab bridge: one Obscura Target per Rustle tab.
// The content WebView of an Obscura tab loads surface.html. This module keeps
// the target/session in sync with the native tab and pushes PNG frames back.

struct NativeTab {
    target: String,
    session: String,
    url: String,
}

struct NativeBridge {
    server: ObscuraServer,
    tabs: HashMap<u64, NativeTab>,
    contexts: HashMap<u64, String>,
}

static NATIVE: Mutex<Option<NativeBridge>> = Mutex::new(None);

fn native_bridge() -> Result<std::sync::MutexGuard<'static, Option<NativeBridge>>, String> {
    NATIVE
        .lock()
        .map_err(|_| "native bridge lock poisoned".to_owned())
}

fn start_native() -> Result<NativeBridge, String> {
    let renderer = ObscuraRenderer::locate().map_err(|error| error.to_string())?;
    let server = ObscuraServer::start(renderer.binary()).map_err(|error| error.to_string())?;
    Ok(NativeBridge {
        server,
        tabs: HashMap::new(),
        contexts: HashMap::new(),
    })
}

pub fn native_ensure(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, url: &str, profile: u64) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if guard.is_none() {
        match start_native() {
            Ok(bridge) => *guard = Some(bridge),
            Err(error) => {
                drop(guard);
                send_native_error(state, tab_id, &error);
                return;
            }
        }
    }
    let Some(bridge) = guard.as_mut() else {
        return;
    };
    if bridge.tabs.contains_key(&tab_id.0) {
        return;
    }
    let context = match bridge.contexts.get(&profile).cloned() {
        Some(context) => context,
        None => match bridge.server.client.create_context() {
            Ok(context) => {
                bridge.contexts.insert(profile, context.clone());
                context
            }
            Err(error) => {
                drop(guard);
                send_native_error(state, tab_id, &error.to_string());
                return;
            }
        },
    };
    let target = match bridge
        .server
        .client
        .create_target("about:blank", Some(&context))
    {
        Ok(target) => target,
        Err(error) => {
            drop(guard);
            send_native_error(state, tab_id, &error.to_string());
            return;
        }
    };
    let session = match bridge.server.client.attach(&target) {
        Ok(session) => session,
        Err(error) => {
            drop(guard);
            send_native_error(state, tab_id, &error.to_string());
            return;
        }
    };
    let _ = bridge
        .server
        .client
        .command("Page.enable", json!({}), Some(&session));
    let _ = bridge
        .server
        .client
        .command("Page.navigate", json!({"url": url}), Some(&session));
    bridge.tabs.insert(
        tab_id.0,
        NativeTab {
            target,
            session,
            url: url.to_owned(),
        },
    );
    std::thread::sleep(Duration::from_millis(900));
    let png = bridge
        .tabs
        .get(&tab_id.0)
        .and_then(|tab| bridge.server.client.screenshot(&tab.session).ok());
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, url);
    }
}

pub fn native_navigate(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, url: &str) {
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(bridge) = guard.as_mut() else {
        return;
    };
    let Some(tab) = bridge.tabs.get_mut(&tab_id.0) else {
        return;
    };
    tab.url = url.to_owned();
    let session = tab.session.clone();
    let _ = bridge
        .server
        .client
        .command("Page.navigate", json!({"url": url}), Some(&session));
    std::thread::sleep(Duration::from_millis(900));
    let png = bridge.server.client.screenshot(&session).ok();
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, url);
    }
}

fn native_history(
    bridge: &mut NativeBridge,
    session: &str,
) -> Result<(Vec<(i64, String)>, usize), String> {
    let value = bridge
        .server
        .client
        .command("Page.getNavigationHistory", json!({}), Some(session))
        .map_err(|error| error.to_string())?;
    let current = value
        .get("currentIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some((
                        entry.get("id").and_then(Value::as_i64)?,
                        entry.get("url").and_then(Value::as_str)?.to_owned(),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((entries, current))
}

fn apply_history_flags(
    state: &Rc<RefCell<AppState>>,
    tab_id: crate::TabId,
    entries: &[(i64, String)],
    current: usize,
) {
    let url = entries.get(current).map(|(_, url)| url.clone());
    if let Some(tab) = state.borrow_mut().tab_mut(tab_id) {
        tab.can_back = current > 0;
        tab.can_forward = current + 1 < entries.len();
        if let Some(url) = url {
            tab.url = url;
        }
    }
}

pub fn native_back(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId) -> bool {
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let Some(bridge) = guard.as_mut() else {
        return false;
    };
    let Some(tab) = bridge.tabs.get(&tab_id.0) else {
        return false;
    };
    let session = tab.session.clone();
    let Ok((entries, current)) = native_history(bridge, &session) else {
        return false;
    };
    if current == 0 {
        return false;
    }
    let Some((entry_id, _)) = entries.get(current - 1) else {
        return false;
    };
    let _ = bridge.server.client.command(
        "Page.navigateToHistoryEntry",
        json!({"entryId": entry_id}),
        Some(&session),
    );
    std::thread::sleep(Duration::from_millis(700));
    let png = bridge.server.client.screenshot(&session).ok();
    let Ok((entries, current)) = native_history(bridge, &session) else {
        return false;
    };
    apply_history_flags(state, tab_id, &entries, current);
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, "back");
    }
    push_ui_state(state);
    true
}

pub fn native_forward(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId) -> bool {
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    let Some(bridge) = guard.as_mut() else {
        return false;
    };
    let Some(tab) = bridge.tabs.get(&tab_id.0) else {
        return false;
    };
    let session = tab.session.clone();
    let Ok((entries, current)) = native_history(bridge, &session) else {
        return false;
    };
    let Some((entry_id, _)) = entries.get(current + 1) else {
        return false;
    };
    let _ = bridge.server.client.command(
        "Page.navigateToHistoryEntry",
        json!({"entryId": entry_id}),
        Some(&session),
    );
    std::thread::sleep(Duration::from_millis(700));
    let png = bridge.server.client.screenshot(&session).ok();
    let Ok((entries, current)) = native_history(bridge, &session) else {
        return false;
    };
    apply_history_flags(state, tab_id, &entries, current);
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, "forward");
    }
    push_ui_state(state);
    true
}

pub fn native_activate(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId) {
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(bridge) = guard.as_mut() else {
        return;
    };
    let Some(tab) = bridge.tabs.get(&tab_id.0) else {
        return;
    };
    let session = tab.session.clone();
    let url = tab.url.clone();
    let _ = bridge.server.client.command(
        "Target.activateTarget",
        json!({"targetId": tab.target}),
        None,
    );
    let png = bridge.server.client.screenshot(&session).ok();
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, &url);
    }
}

pub fn native_reload(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId) {
    let mut guard = match native_bridge() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(bridge) = guard.as_mut() else {
        return;
    };
    let Some(tab) = bridge.tabs.get(&tab_id.0) else {
        return;
    };
    let session = tab.session.clone();
    let url = tab.url.clone();
    let _ = bridge
        .server
        .client
        .command("Page.reload", json!({}), Some(&session));
    std::thread::sleep(Duration::from_millis(700));
    let png = bridge.server.client.screenshot(&session).ok();
    drop(guard);
    if let Some(png) = png {
        send_native_frame(state, tab_id, &png, &url);
    }
}

pub fn native_close(tab_id: crate::TabId) {
    let Ok(mut guard) = native_bridge() else {
        return;
    };
    let Some(bridge) = guard.as_mut() else {
        return;
    };
    if let Some(tab) = bridge.tabs.remove(&tab_id.0) {
        let _ = bridge.server.client.command(
            "Target.closeTarget",
            json!({"targetId": tab.target}),
            None,
        );
    }
}

pub fn native_handle(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, message: &Value) {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "obscura.surface.ready" => native_activate(state, tab_id),
        "obscura.surface.reload" => native_reload(state, tab_id),
        "obscura.surface.tick" => native_activate(state, tab_id),
        "obscura.surface.click" => {
            let x = message.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = message.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let mut guard = match native_bridge() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let Some(bridge) = guard.as_mut() else {
                return;
            };
            let Some(tab) = bridge.tabs.get(&tab_id.0) else {
                return;
            };
            let session = tab.session.clone();
            let _ = bridge.server.client.mouse(&session, x, y);
            std::thread::sleep(Duration::from_millis(250));
            let png = bridge.server.client.screenshot(&session).ok();
            drop(guard);
            if let Some(png) = png {
                send_native_frame(state, tab_id, &png, "click");
            }
        }
        "obscura.surface.key" => {
            let key = message.get("key").and_then(Value::as_str).unwrap_or("");
            let mut guard = match native_bridge() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let Some(bridge) = guard.as_mut() else {
                return;
            };
            let Some(tab) = bridge.tabs.get(&tab_id.0) else {
                return;
            };
            let session = tab.session.clone();
            for kind in ["keyDown", "keyUp"] {
                let _ = bridge.server.client.command(
                    "Input.dispatchKeyEvent",
                    json!({"type": kind, "key": key, "text": key}),
                    Some(&session),
                );
            }
            std::thread::sleep(Duration::from_millis(200));
            let png = bridge.server.client.screenshot(&session).ok();
            drop(guard);
            if let Some(png) = png {
                send_native_frame(state, tab_id, &png, "key");
            }
        }
        _ => {}
    }
}

fn send_native_frame(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, png: &[u8], url: &str) {
    use base64::Engine;
    let screenshot = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let payload = json!({
        "screenshot": screenshot,
        "url": url,
        "status": "Obscura",
    });
    let script = format!(
        "window.rustleObscuraSurface && window.rustleObscuraSurface.update({});",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned())
    );
    if let Some(webview) = state
        .borrow()
        .tab(tab_id)
        .and_then(|tab| tab.webview.as_ref())
    {
        let _ = webview.evaluate_script(&script);
    }
}

fn send_native_error(state: &Rc<RefCell<AppState>>, tab_id: crate::TabId, error: &str) {
    let script = format!(
        "window.rustleObscuraSurface && window.rustleObscuraSurface.update({});",
        serde_json::to_string(&json!({"error": error, "status": error}))
            .unwrap_or_else(|_| "{}".to_owned())
    );
    if let Some(webview) = state
        .borrow()
        .tab(tab_id)
        .and_then(|tab| tab.webview.as_ref())
    {
        let _ = webview.evaluate_script(&script);
    }
}
