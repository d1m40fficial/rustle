use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustle_servo::ServoStaticRenderer;
use tao::platform::windows::WindowExtWindows;

use crate::{AppState, TabId};
use std::cell::RefCell;
use std::rc::Rc;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SERVO_TOP_CROP: i32 = 58;

struct ServoWindow {
    child: Child,
    hwnd: isize,
    url: String,
    visible: bool,
}

static WINDOWS: Mutex<Option<HashMap<TabId, ServoWindow>>> = Mutex::new(None);

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(
        callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
        lparam: isize,
    ) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetWindowTextLengthW(hwnd: isize) -> i32;
    fn SetParent(hwnd: isize, parent: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32)
    -> i32;
    fn ShowWindow(hwnd: isize, command: i32) -> i32;
}

const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_CHILD: isize = 0x4000_0000;
const WS_VISIBLE: isize = 0x1000_0000;
const WS_CAPTION: isize = 0x00C0_0000;
const WS_THICKFRAME: isize = 0x0004_0000;
const WS_POPUP: isize = 0x8000_0000;
const WS_EX_APPWINDOW: isize = 0x0004_0000;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SWP_NOACTIVATE: u32 = 0x0010;
const HWND_BOTTOM: isize = 1;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;

pub fn ensure(state: &Rc<RefCell<AppState>>, tab_id: TabId, url: &str, visible: bool) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let (parent, top, width, height, profile) = {
        let state = state.borrow();
        (
            state.window.hwnd(),
            if state.vertical || state.settings.topbar_style == "compact" {
                44
            } else {
                78
            },
            state.width,
            state.height,
            state.profiles.active,
        )
    };
    let mut guard = match WINDOWS.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let windows = guard.get_or_insert_with(HashMap::new);
    if let Some(window) = windows.get_mut(&tab_id) {
        if window.url == url {
            window.visible = visible;
            resize(window.hwnd, top, width as i32, height as i32, visible);
            return;
        }
        close_window(window);
        windows.remove(&tab_id);
    }
    let Some(binary) = locate_binary() else {
        return;
    };
    let profile_dir = rustle_storage::data_dir()
        .join("servo-profiles")
        .join(profile.to_string());
    let _ = std::fs::create_dir_all(&profile_dir);
    let mut command = Command::new(&binary);
    command
        .arg("-b")
        .arg("--config-dir")
        .arg(&profile_dir)
        .arg(url)
        .current_dir(binary.parent().unwrap_or_else(|| std::path::Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return,
    };
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut hwnd = 0;
    while Instant::now() < deadline {
        hwnd = find_window(pid);
        if hwnd != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if hwnd == 0 {
        let mut child = child;
        let _ = child.kill();
        return;
    }
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let _ = SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            (style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME)) | WS_CHILD | WS_VISIBLE,
        );
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle & !WS_EX_APPWINDOW);
        SetParent(hwnd, parent);
        SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            top - SERVO_TOP_CROP,
            width as i32,
            (height as i32 - top + SERVO_TOP_CROP).max(1),
            SWP_FRAMECHANGED | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
    windows.insert(
        tab_id,
        ServoWindow {
            child,
            hwnd,
            url: url.to_owned(),
            visible,
        },
    );
}

pub fn navigate(state: &Rc<RefCell<AppState>>, tab_id: TabId, url: &str, visible: bool) {
    close(tab_id);
    ensure(state, tab_id, url, visible);
}

pub fn reload(state: &Rc<RefCell<AppState>>, tab_id: TabId) {
    let url = WINDOWS.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .and_then(|windows| windows.get(&tab_id).map(|window| window.url.clone()))
    });
    if let Some(url) = url {
        navigate(state, tab_id, &url, true);
    }
}

pub fn set_visible(tab_id: TabId, visible: bool) {
    let Ok(mut guard) = WINDOWS.lock() else {
        return;
    };
    let Some(windows) = guard.as_mut() else {
        return;
    };
    if let Some(window) = windows.get_mut(&tab_id) {
        window.visible = visible;
        unsafe {
            ShowWindow(window.hwnd, if visible { SW_SHOW } else { SW_HIDE });
        }
    }
}

pub fn close(tab_id: TabId) {
    let Ok(mut guard) = WINDOWS.lock() else {
        return;
    };
    let Some(windows) = guard.as_mut() else {
        return;
    };
    if let Some(mut window) = windows.remove(&tab_id) {
        close_window(&mut window);
    }
}

pub fn layout(state: &Rc<RefCell<AppState>>) {
    let (top, width, height, active) = {
        let state = state.borrow();
        (
            if state.vertical || state.settings.topbar_style == "compact" {
                44
            } else {
                78
            },
            state.width,
            state.height,
            state.active,
        )
    };
    let Ok(mut guard) = WINDOWS.lock() else {
        return;
    };
    let Some(windows) = guard.as_mut() else {
        return;
    };
    for (tab, window) in windows.iter_mut() {
        let visible = *tab == active && window.visible;
        resize(window.hwnd, top, width as i32, height as i32, visible);
    }
}

fn close_window(window: &mut ServoWindow) {
    unsafe {
        ShowWindow(window.hwnd, SW_HIDE);
    }
    let _ = window.child.kill();
    let _ = window.child.wait();
}

fn resize(hwnd: isize, top: i32, width: i32, height: i32, visible: bool) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            top - SERVO_TOP_CROP,
            width,
            (height - top + SERVO_TOP_CROP).max(1),
            SWP_NOACTIVATE | if visible { SWP_SHOWWINDOW } else { 0 },
        );
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}

fn locate_binary() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("RUSTLE_SERVO_BIN") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    ServoStaticRenderer::locate()
        .ok()
        .map(|renderer| renderer.binary().to_path_buf())
}

unsafe extern "system" fn enum_callback(hwnd: isize, lparam: isize) -> i32 {
    unsafe {
        let search = &mut *(lparam as *mut WindowSearch);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == search.pid && IsWindowVisible(hwnd) != 0 && GetWindowTextLengthW(hwnd) > 0 {
            search.hwnd = hwnd;
            return 0;
        }
        1
    }
}

struct WindowSearch {
    pid: u32,
    hwnd: isize,
}

fn find_window(pid: u32) -> isize {
    let mut search = WindowSearch { pid, hwnd: 0 };
    unsafe {
        EnumWindows(
            Some(enum_callback),
            &mut search as *mut WindowSearch as isize,
        );
    }
    search.hwnd
}
