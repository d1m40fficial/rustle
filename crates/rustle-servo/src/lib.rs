//! Servo static-renderer sidecar for Rustle M3.
//!
//! The released servoshell is used as a separate process. This is deliberately
//! a static renderer only: it proves layout/paint parity and gives Rustle a
//! safe path to Servo without embedding the full browser engine into the UI
//! process yet.

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
pub struct ServoStaticRenderer {
    binary: PathBuf,
}

impl ServoStaticRenderer {
    pub fn locate() -> Result<Self, RendererError> {
        let binary = locate_binary().ok_or(RendererError::BinaryNotFound("servoshell"))?;
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
}

impl StaticRenderer for ServoStaticRenderer {
    fn kind(&self) -> RendererKind {
        RendererKind::Servo
    }

    fn render(&self, request: &RenderRequest) -> Result<RenderedPage, RendererError> {
        request.validate()?;
        let started = Instant::now();
        let output_path = temp_png_path();
        let workdir = self
            .binary
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut child = hidden_command(&self.binary)
            .arg("-z")
            .arg("-x")
            .arg("-o")
            .arg(&output_path)
            .arg(format!(
                "--screen-size={}x{}",
                request.width, request.height
            ))
            .arg(&request.url)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(RendererError::Spawn)?;

        wait_for_exit(&mut child, Duration::from_millis(request.timeout_ms))?;
        let bytes = std::fs::read(&output_path).map_err(RendererError::Spawn)?;
        let _ = std::fs::remove_file(&output_path);
        if !png_is_valid(&bytes) {
            return Err(RendererError::EmptyOutput);
        }
        Ok(RenderedPage {
            png: bytes,
            final_url: request.url.clone(),
            elapsed_ms: started.elapsed().as_millis(),
        })
    }
}

fn locate_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RUSTLE_SERVO_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(path) = std::env::current_exe() {
        if let Some(dir) = path.parent() {
            for candidate in [
                dir.join("servoshell.exe"),
                dir.join("tools/servo/bin/servo/servoshell.exe"),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let workspace =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/servo/bin/servo/servoshell.exe");
    if workspace.is_file() {
        return Some(workspace);
    }
    None
}

fn temp_png_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("rustle-servo-{stamp}.png"))
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<(), RendererError> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() {
                return Ok(());
            }
            return Err(RendererError::Failed(format!(
                "servoshell exited: {status}"
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(RendererError::Timeout(timeout.as_millis() as u64))
}
