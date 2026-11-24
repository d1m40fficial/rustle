use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use rustle_obscura::ObscuraRenderer;
use rustle_obscura::cdp::ObscuraServer;

use crate::AppState;
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
