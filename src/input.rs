use std::mem::size_of;

use serde_json::Value;

use crate::ffi::*;
use crate::windows::WindowInfo;

fn send(input: INPUT) -> bool {
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) == 1 }
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

fn mouse(flags: u32, data: u32) -> bool {
    let mut inp: INPUT = unsafe { std::mem::zeroed() };
    inp.r#type = INPUT_MOUSE;
    inp.Anonymous = INPUT_0 {
        mi: MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        },
    };
    send(inp)
}

fn key(vk: u16, scan: u16, flags: u32) -> bool {
    let mut inp: INPUT = unsafe { std::mem::zeroed() };
    inp.r#type = INPUT_KEYBOARD;
    inp.Anonymous = INPUT_0 {
        ki: KEYBDINPUT {
            wVk: vk,
            wScan: scan,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        },
    };
    send(inp)
}

fn focus(w: &WindowInfo) {
    unsafe {
        SetForegroundWindow(w.id as HWND);
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
}

fn is_foreground(w: &WindowInfo) -> bool {
    unsafe { GetForegroundWindow() == w.id as HWND }
}

/// Refuse to press a mouse button on a point that belongs to another app.
/// Clicking blindly is how a "click the QQ input" turns into a click in
/// whatever window happens to be on top.
fn require_point_on(w: &WindowInfo, sx: i32, sy: i32) -> Result<(), String> {
    let Some(under) = crate::windows::window_at(sx, sy) else {
        return Err(format!("nothing under screen point ({sx},{sy}); nothing was clicked"));
    };
    // Same UI family: the target itself, its modal dialogs and its menus.
    // A popup that is its own top-level window (Qt tool windows such as
    // WeChat's search dropdown, Chromium combo lists) has no owner to walk up
    // to, yet it still belongs to this app — clicking it is exactly what the
    // user sees. Another app's window is what this guard exists to refuse.
    if under.id != crate::windows::root_owner(w.id) {
        if under.pid != w.pid {
            return Err(format!(
                "screen point ({sx},{sy}) is over '{}' ({}), not '{}' — the target is covered or not in front; nothing was clicked",
                under.title, under.app, w.title
            ));
        }
    }
    Ok(())
}

pub fn click(w: &WindowInfo, x: Option<i32>, y: Option<i32>, button: &str, count: i32) -> Result<Value, String> {
    let (sx, sy) = match (x, y) {
        (Some(x), Some(y)) => (w.x + x, w.y + y),
        _ => return Err("click needs window-relative x and y".into()),
    };
    focus(w);
    require_point_on(w, sx, sy)?;
    // Report where the click actually landed: a same-process popup (search
    // dropdown, combo list) is the common case and the caller must know.
    let landed = crate::windows::window_at(sx, sy);
    let mut out = click_screen(sx, sy, button, count)?;
    if let Some(l) = landed {
        out["landed_on"] = serde_json::json!({
            "id": l.id,
            "app": l.app,
            "title": l.title,
        });
    }
    Ok(out)
}

/// Click a point in screen coordinates (used for tray icons and name hits).
pub fn click_screen(sx: i32, sy: i32, button: &str, count: i32) -> Result<Value, String> {
    unsafe {
        if SetCursorPos(sx, sy) == 0 {
            return Err("SetCursorPos failed".into());
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    let (down, up) = match button {
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    };
    for _ in 0..count.max(1) {
        if !mouse(down, 0) {
            return Err(format!(
                "mouse-down was blocked (SendInput error {})",
                last_error()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        if !mouse(up, 0) {
            return Err(format!(
                "mouse-up was blocked (SendInput error {})",
                last_error()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    Ok(serde_json::json!({"x": sx, "y": sy, "method": "sendinput"}))
}

pub fn type_text(w: &WindowInfo, text: &str) -> Result<Value, String> {
    focus(w);
    if !is_foreground(w) {
        return Err(format!(
            "{} is not the foreground window; keystrokes would land in another app — call activate first",
            w.title
        ));
    }
    let mut n = 0;
    for ch in text.chars() {
        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf) {
            if !key(0, *unit, KEYEVENTF_UNICODE) || !key(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)
            {
                return Err(format!(
                    "keystroke was blocked after {n} chars (SendInput error {})",
                    last_error()
                ));
            }
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(6));
        }
    }
    Ok(serde_json::json!({"typed": n}))
}

fn vk_for(name: &str) -> Result<u16, String> {
    let lower = name.to_lowercase();
    let v = match lower.as_str() {
        "return" | "enter" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "page_up" | "pageup" => 0x21,
        "page_down" | "pagedown" => 0x22,
        "space" => 0x20,
        "control_l" | "control" | "ctrl" => 0x11,
        "shift_l" | "shift" => 0x10,
        "alt_l" | "alt" => 0x12,
        "super_l" | "win" => 0x5B,
        "f1" => 0x70, "f2" => 0x71, "f3" => 0x72, "f4" => 0x73, "f5" => 0x74,
        "f6" => 0x75, "f7" => 0x76, "f8" => 0x77, "f9" => 0x78, "f10" => 0x79,
        "f11" => 0x7A, "f12" => 0x7B,
        _ => {
            let mut chars = name.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                (c.to_ascii_uppercase() as u16) & 0xFF
            } else {
                return Err(format!("unknown key '{name}'"));
            }
        }
    };
    Ok(v)
}

pub fn press(w: &WindowInfo, chord: &str) -> Result<Value, String> {
    focus(w);
    let parts: Vec<&str> = chord.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    let mut keys = Vec::new();
    for p in &parts {
        keys.push(vk_for(p)?);
    }
    for vk in &keys {
        if !key(*vk, 0, 0) {
            return Err(format!("key-down was blocked (SendInput error {})", last_error()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for vk in keys.iter().rev() {
        if !key(*vk, 0, KEYEVENTF_KEYUP) {
            return Err(format!("key-up was blocked (SendInput error {})", last_error()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(serde_json::json!({"key": chord}))
}

pub fn scroll(w: &WindowInfo, x: i32, y: i32, amount: i32) -> Result<Value, String> {
    focus(w);
    require_point_on(w, w.x + x, w.y + y)?;
    unsafe {
        SetCursorPos(w.x + x, w.y + y);
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    let step: u32 = if amount > 0 { 120 } else { 120u32.wrapping_neg() };
    for _ in 0..amount.abs().max(1) {
        if !mouse(MOUSEEVENTF_WHEEL, step) {
            return Err(format!("wheel was blocked (SendInput error {})", last_error()));
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(serde_json::json!({"scrolled": amount}))
}

pub fn drag(w: &WindowInfo, from: (i32, i32), to: (i32, i32)) -> Result<Value, String> {
    focus(w);
    let (fx, fy) = (w.x + from.0, w.y + from.1);
    let (tx, ty) = (w.x + to.0, w.y + to.1);
    require_point_on(w, fx, fy)?;
    unsafe {
        SetCursorPos(fx, fy);
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    if !mouse(MOUSEEVENTF_LEFTDOWN, 0) {
        return Err(format!("drag mouse-down was blocked (SendInput error {})", last_error()));
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    for i in 1..=12 {
        let nx = fx + (tx - fx) * i / 12;
        let ny = fy + (ty - fy) * i / 12;
        unsafe {
            SetCursorPos(nx, ny);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    if !mouse(MOUSEEVENTF_LEFTUP, 0) {
        return Err(format!("drag mouse-up was blocked (SendInput error {})", last_error()));
    }
    Ok(serde_json::json!({"from": [fx, fy], "to": [tx, ty]}))
}
