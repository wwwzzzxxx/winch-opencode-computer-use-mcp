//! Takeover overlay: screen-edge glow + top pill banner while a batch runs.
//! Visuals follow dsh-computer-use-win's takeover (thin 20px blue edge glow,
//! dark pill, blue dot, Esc keycap). Esc cancels the running batch.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::ffi::*;
use crate::windows::wide;

pub static CANCEL: AtomicBool = AtomicBool::new(false);
static HWND_ATOM: AtomicI64 = AtomicI64::new(0);
static HOOK_ATOM: AtomicI64 = AtomicI64::new(0);
static SHOW_GEN: AtomicU64 = AtomicU64::new(0);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

const WM_CU_SHOW: u32 = WM_APP + 1;
const WM_CU_HIDE: u32 = WM_APP + 2;

pub fn start() {
    let _ = START.set(Instant::now());
    std::thread::spawn(|| unsafe { run_window() });
    // Safety net only: the normal path is hide_after(30s) at the end of each
    // control batch. This watchdog exists for a batch that never returns (or a
    // host that dies), so it must stay well above 30s.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(5));
        if VISIBLE.load(Ordering::SeqCst)
            && now_ms().saturating_sub(LAST_MS.load(Ordering::SeqCst)) > 120_000
        {
            hide_now();
        }
    });
}

pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub fn is_cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

fn post(msg: u32) {
    let h = HWND_ATOM.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            PostMessageW(h as HWND, msg, 0, 0);
        }
    }
}

pub fn show() {
    SHOW_GEN.fetch_add(1, Ordering::SeqCst);
    LAST_MS.store(now_ms(), Ordering::SeqCst);
    VISIBLE.store(true, Ordering::SeqCst);
    post(WM_CU_SHOW);
}

pub fn hide_now() {
    SHOW_GEN.fetch_add(1, Ordering::SeqCst);
    VISIBLE.store(false, Ordering::SeqCst);
    post(WM_CU_HIDE);
}

/// Hide unless another batch starts within `ms`.
pub fn hide_after(ms: u64) {
    let gen = SHOW_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        if SHOW_GEN.load(Ordering::SeqCst) == gen {
            VISIBLE.store(false, Ordering::SeqCst);
            post(WM_CU_HIDE);
        }
    });
}

unsafe extern "system" fn hook_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
        let k = &*(lparam as *const KBDLLHOOKSTRUCT);
        // 0x10 = LLKHF_INJECTED: our own SendInput keystrokes must not stop the
        // batch (only a physical Esc does).
        if k.flags & 0x10 != 0 {
            return CallNextHookEx(HOOK_ATOM.load(Ordering::SeqCst) as HHOOK, code, wparam, lparam);
        }
        if k.vkCode == VK_ESCAPE {
            CANCEL.store(true, Ordering::SeqCst);
            post(WM_CU_HIDE);
            return 1; // swallow Esc so it does not close the user's dialog
        }
    }
    CallNextHookEx(HOOK_ATOM.load(Ordering::SeqCst) as HHOOK, code, wparam, lparam)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_CU_SHOW => {
            paint(hwnd);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(hwnd, HWND_TOPMOST as HWND, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            if HOOK_ATOM.load(Ordering::SeqCst) == 0 {
                let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0);
                if !h.is_null() {
                    HOOK_ATOM.store(h as i64, Ordering::SeqCst);
                }
            }
            0
        }
        WM_CU_HIDE => {
            VISIBLE.store(false, Ordering::SeqCst);
            let h = HOOK_ATOM.swap(0, Ordering::SeqCst);
            if h != 0 {
                UnhookWindowsHookEx(h as HHOOK);
            }
            ShowWindow(hwnd, 0);
            0
        }
        0x0002 => {
            let h = HOOK_ATOM.swap(0, Ordering::SeqCst);
            if h != 0 {
                UnhookWindowsHookEx(h as HHOOK);
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn run_window() {
    let instance = GetModuleHandleW(std::ptr::null());
    let class = wide("CuMcpOverlay");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    RegisterClassW(&wc);

    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(800);
    let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(600);

    let title = wide("winch");
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        vx,
        vy,
        vw,
        vh,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        std::ptr::null_mut(),
    );
    if hwnd.is_null() {
        return;
    }
    HWND_ATOM.store(hwnd as i64, Ordering::SeqCst);

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}

fn set_px(px: &mut [u32], w: i32, h: i32, x: i32, y: i32, color: u32) {
    if x >= 0 && y >= 0 && x < w && y < h {
        px[(y * w + x) as usize] = color;
    }
}

unsafe extern "system" fn monitor_cb(_m: HMONITOR, _dc: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
    if !rect.is_null() {
        (*(data as *mut Vec<RECT>)).push(*rect);
    }
    1
}

fn collect_monitors() -> Vec<RECT> {
    let mut out: Vec<RECT> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(monitor_cb),
            &mut out as *mut _ as LPARAM,
        );
    }
    if out.is_empty() {
        let (pw, ph) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) };
        out.push(RECT {
            left: 0,
            top: 0,
            right: pw,
            bottom: ph,
        });
    }
    out
}

/// dsh style: 2px pale-blue core + 18px #4D7CFE falloff (exponent 2.2),
/// drawn around every monitor so the seam between screens is wrapped too.
fn draw_monitor_glow(px: &mut [u32], w: i32, h: i32, x0: i32, y0: i32, mw: i32, mh: i32) {
    let core = 2i32;
    let glow = core + 18;
    for i in 0..glow {
        let color = if i < core {
            argb(235, 150, 184, 255)
        } else {
            let t = 1.0 - (i - core) as f32 / (glow - core) as f32;
            let a = (175.0 * t.powf(2.2)) as u32;
            argb(a, 77, 124, 254)
        };
        let l = x0 + i;
        let t = y0 + i;
        let r = x0 + mw - 1 - i;
        let b = y0 + mh - 1 - i;
        if r < l || b < t {
            continue;
        }
        for x in l..=r {
            set_px(px, w, h, x, t, color);
            set_px(px, w, h, x, b, color);
        }
        for y in t..=b {
            set_px(px, w, h, l, y, color);
            set_px(px, w, h, r, y, color);
        }
    }
}

fn fill_rrect(px: &mut [u32], w: i32, h: i32, x0: i32, y0: i32, rw: i32, rh: i32, radius: i32, color: u32) {
    for y in y0..(y0 + rh) {
        if y < 0 || y >= h {
            continue;
        }
        for x in x0..(x0 + rw) {
            if x < 0 || x >= w {
                continue;
            }
            let lx = x - x0;
            let ly = y - y0;
            let cx = if lx < radius {
                radius - lx
            } else if lx >= rw - radius {
                lx - (rw - radius - 1)
            } else {
                0
            };
            let cy = if ly < radius {
                radius - ly
            } else if ly >= rh - radius {
                ly - (rh - radius - 1)
            } else {
                0
            };
            if cx > 0 && cy > 0 && (cx * cx + cy * cy) > radius * radius {
                continue;
            }
            px[(y * w + x) as usize] = color;
        }
    }
}

/// GDI text does not write the alpha byte, so any translucent background
/// leaves the glyphs translucent. Force the pill interior fully opaque after
/// drawing the text.
unsafe fn force_opaque(bits: *mut u32, w: i32, h: i32, x0: i32, y0: i32, rw: i32, rh: i32, radius: i32) {
    for y in y0..(y0 + rh) {
        if y < 0 || y >= h {
            continue;
        }
        for x in x0..(x0 + rw) {
            if x < 0 || x >= w {
                continue;
            }
            let lx = x - x0;
            let ly = y - y0;
            let cx = if lx < radius {
                radius - lx
            } else if lx >= rw - radius {
                lx - (rw - radius - 1)
            } else {
                0
            };
            let cy = if ly < radius {
                radius - ly
            } else if ly >= rh - radius {
                ly - (rh - radius - 1)
            } else {
                0
            };
            if cx > 0 && cy > 0 && (cx * cx + cy * cy) > radius * radius {
                continue;
            }
            let p = bits.add((y * w + x) as usize);
            *p |= 0xFF00_0000;
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(800);
    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(600);

    let screen = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return;
    }
    let mem = CreateCompatibleDC(screen);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [0],
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        return;
    }
    let old = SelectObject(mem, bmp);

    let n = (w * h) as usize;
    let mut px: Vec<u32> = vec![0u32; n];
    for m in collect_monitors() {
        draw_monitor_glow(&mut px, w, h, m.left - vx, m.top - vy, m.right - m.left, m.bottom - m.top);
    }

    // --- pill banner, dsh geometry ---
    let bh = 42;
    let bw = 372;
    // Center on the primary monitor, not the virtual desktop bounding box.
    let pw = GetSystemMetrics(0);
    let ph = GetSystemMetrics(1);
    let bx = (pw - bw) / 2 - vx;
    // Top of the primary monitor (user prefers the pill at the top edge).
    let by = 22 - vy;
    // soft shadow (6 rings)
    for i in (1..=6).rev() {
        fill_rrect(&mut px, w, h, bx - i, by - i + 2, bw + 2 * i, bh + 2 * i, bh / 2 + i, argb(9, 0, 0, 0));
    }
    // body + 1px white outline
    fill_rrect(&mut px, w, h, bx - 1, by - 1, bw + 2, bh + 2, bh / 2 + 1, argb(40, 255, 255, 255));
    fill_rrect(&mut px, w, h, bx, by, bw, bh, bh / 2, argb(248, 20, 22, 28));

    // blue dot with halo
    let cy = by + bh / 2;
    let cx = bx + 21;
    for y in (cy - 8)..=(cy + 8) {
        for x in (cx - 8)..=(cx + 8) {
            let dx = x - cx;
            let dy = y - cy;
            let d2 = dx * dx + dy * dy;
            if d2 <= 64 {
                px[(y * w + x) as usize] = argb(64, 86, 134, 254);
            }
        }
    }
    for y in (cy - 4)..=(cy + 4) {
        for x in (cx - 4)..=(cx + 4) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= 16 {
                px[(y * w + x) as usize] = argb(255, 86, 134, 254);
            }
        }
    }

    // divider before the Esc keycap
    let div_x = bx + bw - 78;
    for y in (cy - 9)..=(cy + 9) {
        px[(y * w + div_x) as usize] = argb(56, 255, 255, 255);
    }
    // Esc keycap
    let kw = 52;
    let kh = 24;
    let kx = bx + bw - 64;
    let ky = cy - kh / 2;
    // Opaque keycap: GDI text does not write the alpha byte, so a translucent
    // background would make the label itself translucent.
    fill_rrect(&mut px, w, h, kx - 1, ky - 1, kw + 2, kh + 2, 6, argb(255, 78, 84, 98));
    fill_rrect(&mut px, w, h, kx, ky, kw, kh, 5, argb(255, 48, 52, 62));

    std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, n);

    // --- GDI text ---
    SetBkMode(mem, TRANSPARENT);
    let face = wide("Microsoft YaHei UI");
    let font = CreateFontW(-15, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 4, 0, face.as_ptr());
    let old_font = SelectObject(mem, font);
    SetTextColor(mem, 0x00FA_F7F5); // #F5F7FA
    let mut r1 = RECT {
        left: bx + 36,
        top: by,
        right: bx + bw - 88,
        bottom: by + bh,
    };
    let t1: Vec<u16> = "computer use 正在操控电脑".encode_utf16().collect();
    DrawTextW(mem, t1.as_ptr(), t1.len() as i32, &mut r1, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

    let small = CreateFontW(-12, 0, 0, 0, 500, 0, 0, 0, 0, 0, 0, 4, 0, face.as_ptr());
    let old_small = SelectObject(mem, small);
    SetTextColor(mem, 0x00FA_F7F5);
    let mut r2 = RECT {
        left: kx,
        top: ky,
        right: kx + kw,
        bottom: ky + kh,
    };
    let t2: Vec<u16> = "Esc".encode_utf16().collect();
    DrawTextW(mem, t2.as_ptr(), t2.len() as i32, &mut r2, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

    SelectObject(mem, old_small);
    DeleteObject(small);
    SelectObject(mem, old_font);
    DeleteObject(font);

    force_opaque(bits as *mut u32, w, h, bx, by, bw, bh, bh / 2);

    let size = SIZE { cx: w, cy: h };
    let dst = POINT { x: vx, y: vy };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA,
    };
    UpdateLayeredWindow(hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA);

    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    ReleaseDC(std::ptr::null_mut(), screen);
}
