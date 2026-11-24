//! Minimal synchronous CDP client for Obscura serve.
//!
//! One Rustle UI thread owns the socket; commands are serialized. This is
//! intentionally small and dependency-light so it can be replaced by an
//! in-process Obscura core later without changing the viewer.

use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

use rustle_engine::RendererError;

pub struct ObscuraServer {
    child: Child,
    pub port: u16,
    pub client: CdpClient,
}

impl ObscuraServer {
    pub fn start(binary: &std::path::Path) -> Result<Self, RendererError> {
        let port = free_port().ok_or_else(|| RendererError::Failed("no free port".to_owned()))?;
        let workdir = binary
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let mut child = Command::new(binary)
            .arg("serve")
            .arg("--port")
            .arg(port.to_string())
            .arg("--workers")
            .arg("1")
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(RendererError::Spawn)?;

        let deadline = Instant::now() + Duration::from_secs(12);
        let ws_url = loop {
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RendererError::Timeout(12_000));
            }
            if let Some(url) = version_ws(port) {
                break url;
            }
            thread::sleep(Duration::from_millis(100));
        };
        let client = CdpClient::connect(&ws_url)?;
        Ok(Self {
            child,
            port,
            client,
        })
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ObscuraServer {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct CdpClient {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl CdpClient {
    pub fn connect(url: &str) -> Result<Self, RendererError> {
        let (socket, _) = connect(url).map_err(|error| RendererError::Failed(error.to_string()))?;
        Ok(Self { socket, next_id: 1 })
    }

    pub fn command(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, RendererError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut payload = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            payload["sessionId"] = json!(session);
        }
        let text = serde_json::to_string(&payload)
            .map_err(|error| RendererError::Failed(error.to_string()))?;
        self.socket
            .send(Message::Text(text))
            .map_err(|error| RendererError::Failed(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() > deadline {
                return Err(RendererError::Timeout(20_000));
            }
            let message = self
                .socket
                .read()
                .map_err(|error| RendererError::Failed(error.to_string()))?;
            let text = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Ping(payload) => {
                    let _ = self.socket.send(Message::Pong(payload));
                    continue;
                }
                Message::Close(_) => {
                    return Err(RendererError::Failed("CDP connection closed".to_owned()));
                }
                _ => continue,
            };
            let value: Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(RendererError::Failed(error.to_string()));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn create_target(
        &mut self,
        url: &str,
        context: Option<&str>,
    ) -> Result<String, RendererError> {
        let mut params = json!({"url": url});
        if let Some(context) = context {
            params["browserContextId"] = json!(context);
        }
        let result = self.command("Target.createTarget", params, None)?;
        result
            .get("targetId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                RendererError::Failed("Target.createTarget returned no targetId".to_owned())
            })
    }

    pub fn attach(&mut self, target_id: &str) -> Result<String, RendererError> {
        let result = self.command(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )?;
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                RendererError::Failed("Target.attachToTarget returned no sessionId".to_owned())
            })
    }

    pub fn screenshot(&mut self, session: &str) -> Result<Vec<u8>, RendererError> {
        let result = self.command(
            "Page.captureScreenshot",
            json!({"format": "png", "captureBeyondViewport": false}),
            Some(session),
        )?;
        let data = result.get("data").and_then(Value::as_str).ok_or_else(|| {
            RendererError::Failed("Page.captureScreenshot returned no data".to_owned())
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|error| RendererError::Failed(error.to_string()))
    }

    pub fn mouse(&mut self, session: &str, x: f64, y: f64) -> Result<(), RendererError> {
        self.command(
            "Input.dispatchMouseEvent",
            json!({"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1}),
            Some(session),
        )?;
        self.command(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1}),
            Some(session),
        )?;
        Ok(())
    }

    pub fn create_context(&mut self) -> Result<String, RendererError> {
        let result = self.command("Target.createBrowserContext", json!({}), None)?;
        result
            .get("browserContextId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                RendererError::Failed("Target.createBrowserContext returned no id".to_owned())
            })
    }
}

fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

fn version_ws(port: u16) -> Option<String> {
    let body = ureq::get(&format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_millis(300))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
}
