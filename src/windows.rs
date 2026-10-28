use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::ffi::*;

#[derive(Clone, Debug, Serialize)]
pub struct WindowInfo {
    pub id: i64,
    pub pid: u32,
    pub app: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// False for tray/background windows that enumeration found but the user
    /// cannot see. Input ops refuse these; `show` is the way to bring one back.
    pub visible: bool,
    /// WS_EX_TOOLWINDOW: tray hosts, hotkey sinks and other helper windows.
    /// They can be large, so size alone is not enough to tell them from a UI.
    pub tool_window: bool,
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Full path of a running process's executable.
pub fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn process_name(pid: u32) -> String {
    exe_path(pid)
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| String::from("?"))
}

fn is_tool_window(hwnd: HWND) -> bool {
    unsafe { (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 }
}

/// Window title, bounded in time. `GetWindowTextW` uses the cached caption for
/// other processes (no message, so it cannot block), and the WM_GETTEXT
/// fallback is capped so a hung app still cannot freeze the server.
fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len > 0 {
        return String::from_utf16_lossy(&buf[..len as usize]);
    }
    let mut got: usize = 0;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            buf.len(),
            buf.as_mut_ptr() as isize,
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            100,
            &mut got,
        )
    };
    if ok == 0 || got == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..got.min(buf.len())])
}

/// True when any part of the window is on the virtual screen. Apps often
/// "hide" a window by parking it at coordinates like (12000,12000); making
/// such a window visible is worthless.
fn intersects_virtual_screen(w: &WindowInfo) -> bool {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        w.x < vx + vw && w.x + w.width > vx && w.y < vy + vh && w.y + w.height > vy
    }
}
fn covers_work_area(w: &WindowInfo) -> bool {
    unsafe {
        let mut r = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            &mut r as *mut _ as *mut std::ffi::c_void,
            0,
        ) == 0
        {
            return false;
        }
        w.x <= r.left && w.y <= r.top && w.x + w.width >= r.right && w.y + w.height >= r.bottom
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam as *mut Vec<WindowInfo>);
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return 1;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w < 4 || h < 4 {
        return 1;
    }
    let title = window_title(hwnd);
    let mut cbuf = [0u16; 256];
    let clen = GetClassNameW(hwnd, cbuf.as_mut_ptr(), cbuf.len() as i32);
    let class = String::from_utf16_lossy(&cbuf[..clen.max(0) as usize]);
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    out.push(WindowInfo {
        id: hwnd as i64,
        pid,
        app: process_name(pid),
        title,
        class: Some(class),
        x: rect.left,
        y: rect.top,
        width: w,
        height: h,
        visible: IsWindowVisible(hwnd) != 0,
        tool_window: is_tool_window(hwnd),
    });
    1
}

pub fn list_windows() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc), &mut out as *mut _ as LPARAM);
    }
    out.sort_by(|a, b| (a.app.clone(), a.title.clone()).cmp(&(b.app.clone(), b.title.clone())));
    out
}

/// Visible child windows of `parent` (the tray's XAML host is a child of
/// Shell_TrayWnd, so the top-level scan never sees it).
pub fn child_windows(parent: i64) -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumChildWindows(parent as HWND, Some(enum_proc), &mut out as *mut _ as LPARAM);
    }
    out
}

/// Every top-level window, hidden ones included. Tray apps usually keep
/// their window hidden rather than destroyed, so this is what `show` searches.
pub fn list_windows_all() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc_any), &mut out as *mut _ as LPARAM);
    }
    out
}

unsafe extern "system" fn enum_proc_raw(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam as *mut Vec<WindowInfo>);
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return 1;
    }
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let mut cbuf = [0u16; 256];
    let clen = GetClassNameW(hwnd, cbuf.as_mut_ptr(), cbuf.len() as i32);
    out.push(WindowInfo {
        id: hwnd as i64,
        pid,
        app: process_name(pid),
        title: window_title(hwnd),
        class: Some(String::from_utf16_lossy(&cbuf[..clen.max(0) as usize])),
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        visible: IsWindowVisible(hwnd) != 0,
        tool_window: is_tool_window(hwnd),
    });
    1
}

/// Every top-level window without the size filter: a tray host window is often
/// 0x0 but still owns the notification icon.
pub fn list_windows_raw() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc_raw), &mut out as *mut _ as LPARAM);
    }
    out
}

/// Screen point of one of these processes' notification-area icon, if any.
pub fn tray_icon_point(pids: &[u32]) -> Option<(i32, i32)> {
    for w in list_windows_raw().iter().filter(|w| pids.contains(&w.pid)) {
        for uid in 0..=8u32 {
            let id = NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: w.id as HWND,
                uID: uid,
                guidItem: GUID_ZERO,
            };
            let mut r = RECT::default();
            let hr = unsafe { Shell_NotifyIconGetRect(&id, &mut r) };
            if hr == 0 && r.right > r.left && r.bottom > r.top {
                return Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2));
            }
        }
    }
    None
}

unsafe extern "system" fn enum_proc_any(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam as *mut Vec<WindowInfo>);
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return 1;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w < 4 || h < 4 {
        return 1;
    }
    let title = window_title(hwnd);
    let mut cbuf = [0u16; 256];
    let clen = GetClassNameW(hwnd, cbuf.as_mut_ptr(), cbuf.len() as i32);
    let class = String::from_utf16_lossy(&cbuf[..clen.max(0) as usize]);
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    out.push(WindowInfo {
        id: hwnd as i64,
        pid,
        app: process_name(pid),
        title,
        class: Some(class),
        x: rect.left,
        y: rect.top,
        width: w,
        height: h,
        visible: IsWindowVisible(hwnd) != 0,
        tool_window: is_tool_window(hwnd),
    });
    1
}

pub fn resolve(v: &Value) -> Result<WindowInfo, String> {
    let wins = list_windows();
    if let Some(id) = v.get("id").and_then(Value::as_i64) {
        return wins
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| format!("window id {id} not found (it may have closed)"));
    }
    let needle = v
        .get("title_contains")
        .or_else(|| v.get("title"))
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase());
    if let Some(n) = needle {
        let hits: Vec<WindowInfo> = wins
            .into_iter()
            .filter(|w| w.title.to_lowercase().contains(&n))
            .collect();
        return match hits.len() {
            0 => Err(format!("no window title contains '{n}'")),
            1 => Ok(hits.into_iter().next().unwrap()),
            _ => {
                let list: Vec<String> = hits
                    .iter()
                    .map(|w| format!("{} [{}] ({})", w.title, w.app, w.id))
                    .collect();
                Err(format!("ambiguous title '{n}': {}", list.join("; ")))
            }
        };
    }
    if let Some(a) = v.get("app").and_then(Value::as_str) {
        let a = a.to_lowercase();
        let hits: Vec<WindowInfo> = wins
            .into_iter()
            .filter(|w| w.app.to_lowercase().contains(&a))
            .collect();
        return match hits.len() {
            0 => Err(format!("no window owned by app '{a}'")),
            1 => Ok(hits.into_iter().next().unwrap()),
            _ => {
                let list: Vec<String> = hits
                    .iter()
                    .map(|w| format!("{} [{}] ({})", w.title, w.app, w.id))
                    .collect();
                Err(format!("ambiguous app '{a}': {}", list.join("; ")))
            }
        };
    }
    Err("window reference needs id, title_contains or app".into())
}

/// Pick the most plausible window: never a helper/tool window, then a titled
/// one, then a real-sized one, then visible, then the largest.
fn pick_best(mut hits: Vec<WindowInfo>) -> WindowInfo {
    if hits.iter().any(|w| !w.tool_window) {
        hits.retain(|w| !w.tool_window);
    }
    if hits.iter().any(|w| !w.title.trim().is_empty()) {
        hits.retain(|w| !w.title.trim().is_empty());
    }
    if hits.iter().any(|w| !covers_work_area(w)) {
        hits.retain(|w| !covers_work_area(w));
    }
    if hits.iter().any(|w| w.width >= 64 && w.height >= 64) {
        hits.retain(|w| w.width >= 64 && w.height >= 64);
    }
    hits.sort_by(|a, b| {
        b.visible.cmp(&a.visible).then(
            (b.width as i64 * b.height as i64).cmp(&(a.width as i64 * a.height as i64)),
        )
    });
    hits.remove(0)
}

/// Like `resolve`, but also searches hidden windows: `show` has to find a
/// tray-hidden window that a visible-only scan never reports.
pub fn resolve_any(v: &Value) -> Result<WindowInfo, String> {
    let wins = list_windows_all();
    if let Some(id) = v.get("id").and_then(Value::as_i64) {
        return wins
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| format!("window id {id} not found (visible and hidden windows searched)"));
    }
    let needle = v
        .get("title_contains")
        .or_else(|| v.get("title"))
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase());
    if let Some(n) = needle {
        let hits: Vec<WindowInfo> = wins
            .into_iter()
            .filter(|w| w.title.to_lowercase().contains(&n))
            .collect();
        if hits.is_empty() {
            return Err(format!("no window title contains '{n}' (hidden windows searched too)"));
        }
        return Ok(pick_best(hits));
    }
    if let Some(a) = v.get("app").and_then(Value::as_str) {
        let a = a.to_lowercase();
        let hits: Vec<WindowInfo> = wins
            .into_iter()
            .filter(|w| w.app.to_lowercase().contains(&a))
            .collect();
        if hits.is_empty() {
            return Err(format!("no window owned by app '{a}' (hidden windows searched too)"));
        }
        return Ok(pick_best(hits));
    }
    Err("window reference needs id, title_contains or app".into())
}

/// Hidden windows (tray apps) do not accept input. `show` is the explicit way
/// to restore one; input ops never force windows visible as a side effect.
pub fn ensure_visible(w: &WindowInfo) -> Result<(), String> {
    unsafe {
        if IsWindowVisible(w.id as HWND) == 0 {
            return Err(format!(
                "'{}' is hidden (tray or background). Use {{op:\"show\", window:{{app:\"{}\"}}}} to bring it back; forcing it visible here would leave it painted but deaf to input",
                w.title, w.app
            ));
        }
    }
    Ok(())
}

/// Root owner of a window (itself when unowned). Two windows belong to the
/// same UI family — main window, its dialogs and menus — when this matches.
pub fn root_owner(hwnd: i64) -> i64 {
    unsafe {
        let h = GetAncestor(hwnd as HWND, 3 /* GA_ROOTOWNER */);
        if h.is_null() {
            hwnd
        } else {
            h as i64
        }
    }
}

/// Top-level window under a screen point (respects click-through windows).
pub fn window_at(x: i32, y: i32) -> Option<WindowInfo> {
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.is_null() {
            return None;
        }
        // GA_ROOTOWNER: for an owned popup/menu this walks up to the owning
        // top-level window, so clicks on an app's own menus still pass, while
        // a different top-level window of the same process does not.
        let root = GetAncestor(h, 3 /* GA_ROOTOWNER */);
        let target = if root.is_null() { h } else { root };
        let mut rect = RECT::default();
        if GetWindowRect(target, &mut rect) == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(target, &mut pid);
        let title = window_title(target);
        let mut cbuf = [0u16; 256];
        let clen = GetClassNameW(target, cbuf.as_mut_ptr(), cbuf.len() as i32);
        Some(WindowInfo {
            id: target as i64,
            pid,
            app: process_name(pid),
            title,
            class: Some(String::from_utf16_lossy(&cbuf[..clen.max(0) as usize])),
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            visible: true,
            tool_window: is_tool_window(target),
        })
    }
}

/// PIDs whose executable basename matches `stem` (case-insensitive).
pub fn running_pids(stem: &str) -> Vec<u32> {
    let mut pids = vec![0u32; 8192];
    let mut needed: u32 = 0;
    unsafe {
        if K32EnumProcesses(pids.as_mut_ptr(), (pids.len() * 4) as u32, &mut needed) == 0 {
            return Vec::new();
        }
    }
    let n = ((needed as usize) / 4).min(pids.len());
    let want = stem.to_lowercase();
    let mut out = Vec::new();
    for &pid in &pids[..n] {
        if pid == 0 {
            continue;
        }
        if process_name(pid).to_lowercase() == want {
            out.push(pid);
        }
    }
    out
}

pub fn activate(w: &WindowInfo) -> Result<(), String> {
    ensure_visible(w)?;
    bring_to_front(w.id as HWND)
}

/// Release the foreground lock with a harmless Alt tap, then focus `hwnd`.
fn bring_to_front(hwnd: HWND) -> Result<(), String> {
    unsafe {
        // Tapping Alt releases the foreground lock for a background process.
        keybd_event(0x12, 0, 0, 0);
        keybd_event(0x12, 0, 2, 0);
        // The key-up can be lost while the foreground window changes, leaving
        // Alt logically down; every later click would be Alt+click and every
        // keystroke swallowed. Release both Alt variants unconditionally.
        keybd_event(0xA4, 0, 2, 0);
        keybd_event(0xA5, 0, 2, 0);
        std::thread::sleep(std::time::Duration::from_millis(40));
        if SetForegroundWindow(hwnd) == 0 {
            return Err("SetForegroundWindow failed (Windows foreground lock)".into());
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    Ok(())
}

/// Chromium/Electron windows (QQ, WeChat, aTrust) tear their renderer down
/// when they hide. ShowWindow then resurrects only the native frame: it paints
/// the last frame (so PrintWindow reports "rendered") but never accepts input.
/// Those windows must be reopened by the app itself.
fn is_chromium_class(class: &str) -> bool {
    class.starts_with("Chrome_WidgetWin")
}

fn leaf_of(app: &str) -> String {
    std::path::Path::new(app)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| app.to_lowercase())
}

/// Restore a hidden or minimized window and verify the result. Applied only
/// when the caller explicitly asks for `show`; reading state never does this.
pub fn show_window(w: &WindowInfo) -> Result<Value, String> {
    let hwnd = w.id as HWND;
    // A hidden app can be power-throttled or suspended, so the first request
    // may only wake it; retry instead of trusting one call.
    let mut visible = false;
    for _ in 0..3 {
        unsafe {
            // Async on purpose: ShowWindow blocks forever when the target's UI
            // thread is hung, and one unresponsive app must not freeze the
            // whole server. Poll the result instead.
            if IsIconic(hwnd) != 0 {
                ShowWindowAsync(hwnd, SW_RESTORE);
            }
            if IsWindowVisible(hwnd) == 0 {
                ShowWindowAsync(hwnd, SW_SHOW);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            visible = true;
            break;
        }
    }
    if !visible {
        return Err(format!(
            "'{}' did not become visible after 3 ShowWindow attempts; the app may only restore it from its own UI",
            w.title
        ));
    }
    // Focus is best effort: the window is restored either way.
    let _ = bring_to_front(hwnd);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut shown = list_windows_all()
        .into_iter()
        .find(|x| x.id == w.id)
        .unwrap_or_else(|| w.clone());
    // Some apps hide a window by parking it far outside the desktop, so
    // SW_SHOW alone leaves it invisible. Pull it into the work area.
    let mut moved = false;
    if !intersects_virtual_screen(&shown) {
        let mut r = RECT::default();
        let (mut mx, mut my) = (60, 60);
        unsafe {
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                &mut r as *mut _ as *mut std::ffi::c_void,
                0,
            ) != 0
            {
                mx = r.left + 60;
                my = r.top + 60;
            }
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                mx,
                my,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        if let Some(x) = list_windows_all().into_iter().find(|x| x.id == w.id) {
            shown = x;
        }
        moved = true;
    }
    // A window that is visible but paints nothing is an empty shell: Electron
    // apps often tear down their web view when they hide, and ShowWindow then
    // resurrects only the native frame. Callers must not treat that as success.
    let rendered = crate::capture::renders(shown.id, shown.width, shown.height);
    let mut out = serde_json::json!({
        "via": "show-window",
        "window": shown,
        "moved_on_screen": moved,
        "rendered": rendered,
    });
    if !rendered {
        out["hint"] = serde_json::json!(
            "the window is visible but paints nothing (its content was torn down); reopen it through the app itself — {op:\"show\", window:{app:\"...\"}} re-runs the exe, or use its tray icon"
        );
    } else if is_chromium_class(shown.class.as_deref().unwrap_or("")) {
        out["warning"] = serde_json::json!(
            "chromium window restored by ShowWindow: it may paint the last frame but ignore input. If clicks do nothing, call {op:\"show\", window:{app:\"...\"}} once, then {op:\"launch\", app:\"<exe path>\", restart:true} to restart that app"
        );
    }
    Ok(out)
}

/// Make a hidden window visible, whatever hides it:
///   1. a window that still exists but is hidden or minimized → restore it;
///   2. the app refuses to show it in place, or has no window → run the exe
///      once, which single-instance apps treat as "show yourself".
/// Nothing is ever killed here.
pub fn show_ref(v: &Value) -> Result<Value, String> {
    if let Some(app) = v.get("app").and_then(Value::as_str) {
        return reactivate(app);
    }
    let w = resolve_any(v)?;
    // A hidden Chromium window must be reopened by its own app: ShowWindow by
    // id is exactly how a live-but-blank frame gets resurrected.
    if !w.visible && is_chromium_class(w.class.as_deref().unwrap_or("")) {
        if let Ok(reopened) = reactivate(&w.app) {
            let same_window = reopened
                .get("window")
                .and_then(|x| x.get("id"))
                .and_then(Value::as_i64)
                == Some(w.id);
            let same_process = reopened
                .get("window")
                .and_then(|x| x.get("pid"))
                .and_then(Value::as_u64)
                == Some(w.pid as u64);
            if same_window || same_process {
                return Ok(reopened);
            }
        }
    }
    show_window(&w)
}

/// Ask an app to bring its window back. Generic for any single-instance app:
/// a hidden window is restored in place; if that fails or no window exists,
/// running the exe again makes the running instance show its window.
pub fn reactivate(app: &str) -> Result<Value, String> {
    let leaf = leaf_of(app);
    let pids = running_pids(&leaf);

    // 0. Nothing visible: click the app's own tray icon, exactly like a human
    //    would. The app then rebuilds its window, instead of ShowWindow
    //    resurrecting an empty shell. Skipped when a window is already visible
    //    so a toggle-style icon cannot hide it.
    let any_visible = list_windows_all().iter().any(|w| {
        w.visible
            && !w.tool_window
            && w.width >= 64
            && w.height >= 64
            && w.app.to_lowercase().contains(&leaf)
    });
    if !any_visible && !pids.is_empty() {
        // The icon may live in the hidden-icons flyout: the first click then
        // only opens that flyout, so probe the rect again and click a second
        // time before giving up.
        for _attempt in 0..2 {
            let Some((tx, ty)) = tray_icon_point(&pids) else { break };
            if crate::input::click_screen(tx, ty, "left", 1).is_err() {
                break;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1800);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let ws: Vec<WindowInfo> = list_windows_all()
                    .into_iter()
                    .filter(|w| {
                        w.visible
                            && !w.tool_window
                            && w.width >= 64
                            && w.height >= 64
                            && w.app.to_lowercase().contains(&leaf)
                    })
                    .collect();
                if !ws.is_empty() {
                    let w = pick_best(ws);
                    let rendered = crate::capture::renders(w.id, w.width, w.height);
                    let _ = bring_to_front(w.id as HWND);
                    return Ok(serde_json::json!({
                        "via": "tray-icon",
                        "window": w,
                        "rendered": rendered,
                    }));
                }
            }
        }
    }

    // 1. A real window already exists, hidden or minimized. Helper windows
    //    (WS_EX_TOOLWINDOW) are never a substitute: showing one "succeeds"
    //    while the user still sees nothing.
    let mut cands: Vec<WindowInfo> = list_windows_all()
        .into_iter()
        .filter(|w| {
            !leaf.is_empty()
                && w.app.to_lowercase().contains(&leaf)
                && !w.tool_window
                && w.width >= 64
                && w.height >= 64
        })
        .collect();
    let mut known_pids = pids.clone();
    let mut exe_hint: Option<String> = None;
    if !cands.is_empty() {
        let best = pick_best(cands);
        if !known_pids.contains(&best.pid) {
            known_pids.push(best.pid);
        }
        // The window's process is the authoritative source for what to run
        // again: the display name ("aTrustTray") need not match the app hint
        // the caller used ("aTrust").
        exe_hint = exe_path(best.pid);
        // A minimized window is safe to restore. A *hidden* Chromium window is
        // not: ShowWindow brings back a frame whose renderer was torn down, and
        // the result is visible, screenshots fine, and ignores every click.
        // Leave that case to step 2, which asks the running instance itself.
        if best.visible || !is_chromium_class(best.class.as_deref().unwrap_or("")) {
            if let Ok(v) = show_window(&best) {
                if v.get("rendered").and_then(Value::as_bool).unwrap_or(true) {
                    return Ok(v);
                }
                // Visible but blank (the app tore its content down): fall
                // through and ask the app itself to reopen the window.
            }
        }
        // The window exists but the app refuses to show it in place; fall
        // through and ask the running instance again.
    }

    // 2. No usable window: re-run the exe. Do not kill the running instance.
    let exe = pids
        .iter()
        .filter_map(|p| exe_path(*p))
        .find(|p| std::path::Path::new(p).is_file())
        .or_else(|| exe_hint.filter(|p| std::path::Path::new(p).is_file()))
        .unwrap_or_else(|| app.to_string());
    Command::new(&exe)
        .spawn()
        .map_err(|e| format!("spawn {exe}: {e}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut ws: Vec<WindowInfo> = list_windows_all()
            .into_iter()
            .filter(|w| {
                w.visible
                    && !leaf.is_empty()
                    && w.app.to_lowercase().contains(&leaf)
                    && !w.tool_window
                    && w.width >= 64
                    && w.height >= 64
            })
            .collect();
        if ws.iter().any(|w| !w.title.trim().is_empty()) {
            ws.retain(|w| !w.title.trim().is_empty());
        }
        if !ws.is_empty() {
            let w = pick_best(ws.drain(..).collect());
            if crate::capture::renders(w.id, w.width, w.height) {
                let _ = bring_to_front(w.id as HWND);
                return Ok(serde_json::json!({"via": "relaunch", "window": w, "rendered": true}));
            }
        }
    }
    Err(format!(
        "{leaf} is running (pids {:?}) but showed no rendering window within 8s; it may only open from its own tray menu",
        known_pids
    ))
}

pub fn launch(app: &str, restart: bool, bring_back: bool) -> Result<Value, String> {
    let leaf = leaf_of(app);
    let mut pids = running_pids(&leaf);
    if !pids.is_empty() && restart {
        // Explicit last resort: stop the existing instance, then start one
        // fresh. Only the caller can ask for this; never implicit.
        for pid in &pids {
            unsafe {
                let h = OpenProcess(0x0001, 0, *pid); // PROCESS_TERMINATE
                if !h.is_null() {
                    TerminateProcess(h, 1);
                    CloseHandle(h);
                }
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        while std::time::Instant::now() < deadline && !running_pids(&leaf).is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        pids = running_pids(&leaf);
        if !pids.is_empty() {
            return Err(format!(
                "could not stop {leaf} (pids {:?} still alive); refusing to start a second instance",
                pids
            ));
        }
    }
    if !pids.is_empty() {
        if bring_back {
            return reactivate(app);
        }
        // Single-instance apps (QQ, WeChat, browsers) hand the UI host to the
        // new process when relaunched; the old window keeps painting but stops
        // accepting input. Never spawn a second instance.
        //
        // "Has a window" must mean a real UI window: tray apps leave tiny shim
        // windows (16x16, empty title) that are visible but useless.
        let visible = list_windows().iter().any(|w| {
            w.app.to_lowercase() == leaf
                && !w.title.trim().is_empty()
                && w.width >= 64
                && w.height >= 64
        });
        return Ok(serde_json::json!({
            "launched": false,
            "already_running": true,
            "pids": pids,
            "has_visible_window": visible,
            "hint": if visible {
                "Already running — use activate/state on the existing window."
            } else {
                "Already running with no usable window (hidden to tray or background). Use {op:\"show\", window:{app:\"<name>\"}}: it restores a hidden window, or re-runs the exe so a single-instance app shows itself. reactivate:true does the same from launch."
            }
        }));
    }
    let before: Vec<i64> = list_windows().iter().map(|w| w.id).collect();
    Command::new(app)
        .spawn()
        .map_err(|e| format!("spawn {app}: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if let Some(w) = list_windows()
            .into_iter()
            .find(|w| !before.contains(&w.id) && !w.title.trim().is_empty())
        {
            let _ = activate(&w);
            return Ok(serde_json::to_value(&w).unwrap());
        }
    }
    if let Some(w) = list_windows()
        .into_iter()
        .find(|w| !leaf.is_empty() && w.app.to_lowercase().contains(&leaf))
    {
        let _ = activate(&w);
        return Ok(serde_json::to_value(&w).unwrap());
    }
    Ok(Value::Null)
}

pub fn close(w: &WindowInfo) -> Result<(), String> {
    unsafe {
        ShowWindowAsync(w.id as HWND, 0);
    }
    Ok(())
}
