//! Obscura sidecar probe for Rustle M1.
//!
//! This intentionally uses the released CLI instead of linking Obscura's
//! workspace: it keeps the browser UI independent from V8 and lets the sidecar
//! crash without taking Rustle down. A later milestone can replace this adapter
//! with the in-process browser core behind the same interface.

pub mod cdp;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hidden_command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustle_engine::{
    RenderRequest, RenderedPage, RendererError, RendererKind, StaticRenderer, png_is_valid,
};

#[derive(Debug, Clone)]
pub struct ObscuraRenderer {
    binary: PathBuf,
}

impl ObscuraRenderer {
    pub fn locate() -> Result<Self, RendererError> {
        let binary = locate_binary().ok_or(RendererError::BinaryNotFound("obscura"))?;
        Ok(Self { binary })
    }

    pub fn from_binary(path: impl Into<PathBuf>) -> Self {
        Self {
            binary: path.into(),
        }
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn capture_text(&self, request: &RenderRequest) -> Result<String, RendererError> {
        request.validate()?;
        let mut command = hidden_command(&self.binary);
        command
            .arg("fetch")
            .arg(&request.url)
            .arg("--dump")
            .arg("text")
            .arg("--timeout")
            .arg((request.timeout_ms / 1000).max(1).to_string())
            .arg("--quiet")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().map_err(RendererError::Spawn)?;
        if !output.status.success() {
            return Err(RendererError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn capture_png(&self, request: &RenderRequest) -> Result<Vec<u8>, RendererError> {
        request.validate()?;
        let output_path = temp_png_path("obscura");
        let timeout = Duration::from_millis(request.timeout_ms);
        let mut child = hidden_command(&self.binary)
            .arg("fetch")
            .arg(&request.url)
            .arg("--screenshot")
            .arg(&output_path)
            .arg("--timeout")
            .arg((request.timeout_ms / 1000).max(1).to_string())
            .arg("--quiet")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(RendererError::Spawn)?;

        wait_for_exit_or_file(&mut child, &output_path, timeout)?;
        let bytes = std::fs::read(&output_path).map_err(RendererError::Spawn)?;
        let _ = std::fs::remove_file(&output_path);
        if !png_is_valid(&bytes) {
            return Err(RendererError::EmptyOutput);
        }
        Ok(bytes)
    }
}

impl StaticRenderer for ObscuraRenderer {
    fn kind(&self) -> RendererKind {
        RendererKind::Obscura
    }

    fn render(&self, request: &RenderRequest) -> Result<RenderedPage, RendererError> {
        let started = Instant::now();
        let png = self.capture_png(request)?;
        Ok(RenderedPage {
            png,
            final_url: request.url.clone(),
            elapsed_ms: started.elapsed().as_millis(),
        })
    }
}

fn locate_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RUSTLE_OBSCURA_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(path) = std::env::current_exe() {
        if let Some(dir) = path.parent() {
            for candidate in [
                dir.join("obscura.exe"),
                dir.join("tools/obscura/bin/obscura.exe"),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let workspace =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/obscura/bin/obscura.exe");
    if workspace.is_file() {
        return Some(workspace);
    }
    None
}

fn temp_png_path(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("rustle-{prefix}-{stamp}.png"))
}

fn wait_for_exit_or_file(
    child: &mut Child,
    path: &Path,
    timeout: Duration,
) -> Result<(), RendererError> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(());
        }
        if path.is_file() {
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 8 {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(());
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(RendererError::Timeout(timeout.as_millis() as u64))
}
