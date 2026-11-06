//! Single-instance guard and loopback IPC for forwarding URLs to the running app.

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const MUTEX_NAME: &str = "Local\\Rustle.SingleInstance";

fn ipc_path() -> PathBuf {
    rustle_storage::data_dir().join("ipc.json")
}

/// Returns true when this process owns the single-instance mutex.
#[cfg(windows)]
pub fn acquire() -> bool {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;

    let name: Vec<u16> = MUTEX_NAME.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle == 0 {
        return true;
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        false
    } else {
        // Keep the handle alive for the process lifetime; Windows releases it on exit.
        true
    }
}

#[cfg(not(windows))]
pub fn acquire() -> bool {
    true
}

/// Keeps the IPC endpoint file alive for the process lifetime.
pub struct IpcServer {
    path: PathBuf,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Start a loopback listener and forward each received line to the callback.
pub fn start<F>(on_message: F) -> Option<IpcServer>
where
    F: Fn(String) + Send + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    );

    let path = ipc_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let info = json!({ "port": port, "token": token });
    fs::write(&path, serde_json::to_vec(&info).ok()?).ok()?;

    let expected_token = token;
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut payload = String::new();
            let _ = stream.read_to_string(&mut payload);
            for line in payload.lines() {
                let Some((token, message)) = line.split_once('\t') else {
                    continue;
                };
                if token == expected_token && !message.is_empty() {
                    on_message(message.to_owned());
                }
            }
        }
    });

    Some(IpcServer { path })
}

/// Try to send one or more messages to the already running instance.
pub fn notify_existing(message: &str) -> bool {
    let path = ipc_path();

    for _ in 0..6 {
        let Ok(bytes) = fs::read(&path) else {
            thread::sleep(Duration::from_millis(150));
            continue;
        };
        let Ok(info) = serde_json::from_slice::<Value>(&bytes) else {
            return false;
        };
        let port = info.get("port").and_then(Value::as_u64);
        let token = info.get("token").and_then(Value::as_str);
        let (Some(port), Some(token)) = (port, token) else {
            return false;
        };

        let address = format!("127.0.0.1:{port}");
        if let Ok(mut stream) = TcpStream::connect_timeout(
            &address.parse().expect("valid loopback address"),
            Duration::from_millis(500),
        ) {
            for line in message.lines() {
                let _ = writeln!(stream, "{token}\t{line}");
            }
            let _ = stream.flush();
            return true;
        }
        thread::sleep(Duration::from_millis(150));
    }

    false
}

/// Command-line arguments that should be treated as URLs, not flags.
pub fn startup_urls() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with("--"))
        .collect()
}

/// Payload for a second instance: URL arguments or a focus request.
pub fn payload_from_args() -> String {
    let urls = startup_urls();
    if urls.is_empty() {
        "__focus__".to_owned()
    } else {
        urls.join("\n")
    }
}
