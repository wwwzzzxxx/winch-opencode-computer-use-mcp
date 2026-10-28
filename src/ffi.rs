//! Minimal hand-written Win32 FFI. We avoid windows-sys because its raw-dylib
//! imports require dlltool, which a bare rustup GNU toolchain does not have.
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type BOOL = i32;
pub type HWND = *mut c_void;
pub type HDC = *mut c_void;
pub type HBITMAP = *mut c_void;
pub type HGDIOBJ = *mut c_void;
pub type HANDLE = *mut c_void;
pub type HMODULE = *mut c_void;
pub type LPARAM = isize;
pub type LPCWSTR = *const u16;

pub const SW_SHOW: i32 = 5;
pub const SW_RESTORE: i32 = 9;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
pub const INPUT_MOUSE: u32 = 0;
pub const INPUT_KEYBOARD: u32 = 1;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_UNICODE: u32 = 0x0004;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MOUSEINPUT {
    pub dx: i32,
    pub dy: i32,
    pub mouseData: u32,
    pub dwFlags: u32,
    pub time: u32,
    pub dwExtraInfo: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEYBDINPUT {
    pub wVk: u16,
    pub wScan: u16,
    pub dwFlags: u32,
    pub time: u32,
    pub dwExtraInfo: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HARDWAREINPUT {
    pub uMsg: u32,
    pub wParamL: u16,
    pub wParamH: u16,
}

#[repr(C)]
pub union INPUT_0 {
    pub mi: MOUSEINPUT,
    pub ki: KEYBDINPUT,
    pub hi: HARDWAREINPUT,
}

#[repr(C)]
pub struct INPUT {
    pub r#type: u32,
    pub Anonymous: INPUT_0,
}

pub type WNDENUMPROC = Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>;
pub type HMONITOR = *mut c_void;
pub type MONITORENUMPROC = Option<unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> BOOL>;

#[link(name = "kernel32")]
extern "system" {
    pub fn OpenProcess(access: u32, inherit: BOOL, pid: u32) -> HANDLE;
    pub fn TerminateProcess(h: HANDLE, code: u32) -> BOOL;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn QueryFullProcessImageNameW(h: HANDLE, flags: u32, buf: *mut u16, size: *mut u32) -> BOOL;
    pub fn K32EnumProcesses(pids: *mut u32, cb: u32, needed: *mut u32) -> BOOL;
    pub fn GetLastError() -> u32;
    pub fn LoadLibraryW(name: LPCWSTR) -> HMODULE;
    pub fn GetProcAddress(m: HMODULE, name: *const u8) -> *mut c_void;
    pub fn SetProcessDPIAware() -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn EnumWindows(cb: WNDENUMPROC, l: LPARAM) -> BOOL;
    pub fn EnumChildWindows(parent: HWND, cb: WNDENUMPROC, l: LPARAM) -> BOOL;
    pub fn EnumDisplayMonitors(dc: HDC, clip: *const RECT, cb: MONITORENUMPROC, data: LPARAM) -> BOOL;
    pub fn IsWindowVisible(h: HWND) -> BOOL;
    pub fn IsWindow(h: HWND) -> BOOL;
    pub fn IsIconic(h: HWND) -> BOOL;
    pub fn GetWindowRect(h: HWND, r: *mut RECT) -> BOOL;
    pub fn GetWindowTextW(h: HWND, buf: *mut u16, n: i32) -> i32;
    pub fn SendMessageTimeoutW(
        h: HWND,
        msg: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
    pub fn GetClassNameW(h: HWND, buf: *mut u16, n: i32) -> i32;
    pub fn GetWindowThreadProcessId(h: HWND, pid: *mut u32) -> u32;
    pub fn GetWindowLongPtrW(h: HWND, index: i32) -> isize;
    pub fn SetForegroundWindow(h: HWND) -> BOOL;
    pub fn GetForegroundWindow() -> HWND;
    pub fn ShowWindow(h: HWND, cmd: i32) -> BOOL;
    pub fn ShowWindowAsync(h: HWND, cmd: i32) -> BOOL;
    pub fn SetCursorPos(x: i32, y: i32) -> BOOL;
    pub fn WindowFromPoint(pt: POINT) -> HWND;
    pub fn GetAncestor(h: HWND, flags: u32) -> HWND;
    pub fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    pub fn SendInput(n: u32, inputs: *const INPUT, size: i32) -> u32;
    pub fn GetDC(h: HWND) -> HDC;
    pub fn ReleaseDC(h: HWND, dc: HDC) -> i32;
    pub fn PrintWindow(h: HWND, dc: HDC, flags: u32) -> BOOL;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, winini: u32) -> BOOL;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(dc: HDC) -> HDC;
    pub fn CreateCompatibleBitmap(dc: HDC, w: i32, h: i32) -> HBITMAP;
    pub fn SelectObject(dc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn DeleteObject(obj: HGDIOBJ) -> BOOL;
    pub fn DeleteDC(dc: HDC) -> BOOL;
    pub fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, rop: u32) -> BOOL;
    pub fn StretchBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, sx: i32, sy: i32, sw: i32, sh: i32, rop: u32) -> BOOL;
    pub fn GetPixel(dc: HDC, x: i32, y: i32) -> u32;
}

// ---- GDI+ is loaded dynamically: rustup's GNU toolchain ships libgdi32.a but
// no libgdiplus.a, so we cannot link it directly.

#[repr(C)]
pub struct GdiplusStartupInput {
    pub GdiplusVersion: u32,
    pub DebugEventCallback: usize,
    pub SuppressBackgroundThread: BOOL,
    pub SuppressExternalCodecs: BOOL,
}

pub struct GdiplusApi {
    pub startup:
        unsafe extern "system" fn(*mut usize, *const GdiplusStartupInput, *mut *mut c_void) -> i32,
    pub create_from_hbitmap:
        unsafe extern "system" fn(HBITMAP, *mut c_void, *mut *mut c_void) -> i32,
    pub save_to_file:
        unsafe extern "system" fn(*mut c_void, LPCWSTR, *const GUID, *const c_void) -> i32,
    pub dispose: unsafe extern "system" fn(*mut c_void) -> i32,
}

unsafe impl Send for GdiplusApi {}
unsafe impl Sync for GdiplusApi {}

pub fn gdiplus() -> Option<&'static GdiplusApi> {
    use std::sync::OnceLock;
    static API: OnceLock<Option<GdiplusApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let name: Vec<u16> = "gdiplus.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() {
            return None;
        }
        let sym = |n: &str| -> *mut c_void {
            match std::ffi::CString::new(n) {
                Ok(c) => GetProcAddress(module, c.as_ptr() as *const u8),
                Err(_) => std::ptr::null_mut(),
            }
        };
        let startup = sym("GdiplusStartup");
        let create = sym("GdipCreateBitmapFromHBITMAP");
        let save = sym("GdipSaveImageToFile");
        let dispose = sym("GdipDisposeImage");
        if startup.is_null() || create.is_null() || save.is_null() || dispose.is_null() {
            return None;
        }
        Some(GdiplusApi {
            startup: std::mem::transmute(startup),
            create_from_hbitmap: std::mem::transmute(create),
            save_to_file: std::mem::transmute(save),
            dispose: std::mem::transmute(dispose),
        })
    })
    .as_ref()
}

// ---- Overlay window (takeover indicator) ----

pub type HHOOK = *mut c_void;
pub type HFONT = *mut c_void;
pub type HBRUSH = *mut c_void;

pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_EX_LAYERED: u32 = 0x0008_0000;
pub const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const GWL_EXSTYLE: i32 = -20;
pub const SPI_GETWORKAREA: u32 = 0x0030;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const ULW_ALPHA: u32 = 2;
pub const AC_SRC_OVER: u8 = 0;
pub const AC_SRC_ALPHA: u8 = 1;
pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;
pub const WH_KEYBOARD_LL: i32 = 13;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_GETTEXT: u32 = 0x000D;
pub const SMTO_BLOCK: u32 = 0x0001;
pub const SMTO_ABORTIFHUNG: u32 = 0x0002;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const VK_ESCAPE: u32 = 0x1B;
pub const DIB_RGB_COLORS: u32 = 0;
pub const BI_RGB: u32 = 0;
pub const DT_CENTER: u32 = 0x0000_0001;
pub const DT_VCENTER: u32 = 0x0000_0004;
pub const DT_SINGLELINE: u32 = 0x0000_0020;
pub const TRANSPARENT: i32 = 1;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_ASYNCWINDOWPOS: u32 = 0x4000;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const HWND_TOPMOST: isize = -1;
pub const WM_APP: u32 = 0x8000;
pub const WS_VISIBLE: u32 = 0x1000_0000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}

#[repr(C)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [u32; 1],
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: usize,
    pub lParam: isize,
    pub time: u32,
    pub pt: POINT,
}

#[repr(C)]
pub struct KBDLLHOOKSTRUCT {
    pub vkCode: u32,
    pub scanCode: u32,
    pub flags: u32,
    pub time: u32,
    pub dwExtraInfo: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NOTIFYICONIDENTIFIER {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub guidItem: GUID,
}

pub const GUID_ZERO: GUID = GUID {
    data1: 0,
    data2: 0,
    data3: 0,
    data4: [0; 8],
};

#[link(name = "shell32")]
extern "system" {
    /// Screen rect of a notification-area icon. Returns S_OK (0) when found.
    pub fn Shell_NotifyIconGetRect(
        identifier: *const NOTIFYICONIDENTIFIER,
        icon_location: *mut RECT,
    ) -> i32;
}

pub type WNDPROC = Option<unsafe extern "system" fn(HWND, u32, usize, isize) -> isize>;
pub type HOOKPROC = Option<unsafe extern "system" fn(i32, usize, isize) -> isize>;

#[repr(C)]
pub struct WNDCLASSW {
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HMODULE,
    pub hIcon: *mut c_void,
    pub hCursor: *mut c_void,
    pub hbrBackground: *mut c_void,
    pub lpszMenuName: LPCWSTR,
    pub lpszClassName: LPCWSTR,
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassW(c: *const WNDCLASSW) -> u16;
    pub fn CreateWindowExW(
        ex: u32,
        class: LPCWSTR,
        name: LPCWSTR,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: *mut c_void,
        instance: HMODULE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DefWindowProcW(h: HWND, msg: u32, w: usize, l: isize) -> isize;
    pub fn DestroyWindow(h: HWND) -> BOOL;
    pub fn UpdateLayeredWindow(
        h: HWND,
        dst: HDC,
        dst_pt: *const POINT,
        size: *const SIZE,
        src: HDC,
        src_pt: *const POINT,
        color: u32,
        blend: *const BLENDFUNCTION,
        flags: u32,
    ) -> BOOL;
    pub fn GetMessageW(m: *mut MSG, h: HWND, min: u32, max: u32) -> i32;
    pub fn TranslateMessage(m: *const MSG) -> BOOL;
    pub fn DispatchMessageW(m: *const MSG) -> isize;
    pub fn PostMessageW(h: HWND, msg: u32, w: usize, l: isize) -> BOOL;
    pub fn PostQuitMessage(code: i32);
    pub fn SetWindowsHookExW(id: i32, proc: HOOKPROC, module: HMODULE, thread: u32) -> HHOOK;
    pub fn UnhookWindowsHookEx(h: HHOOK) -> BOOL;
    pub fn CallNextHookEx(h: HHOOK, code: i32, w: usize, l: isize) -> isize;
    pub fn SetWindowPos(h: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> BOOL;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: LPCWSTR) -> HMODULE;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateDIBSection(
        dc: HDC,
        info: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut c_void,
        section: HANDLE,
        offset: u32,
    ) -> HBITMAP;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch: u32,
        face: LPCWSTR,
    ) -> HFONT;
    pub fn SetBkMode(dc: HDC, mode: i32) -> i32;
    pub fn SetTextColor(dc: HDC, color: u32) -> u32;
    pub fn DrawTextW(dc: HDC, text: LPCWSTR, len: i32, rect: *mut RECT, format: u32) -> i32;
    pub fn CreateSolidBrush(color: u32) -> HBRUSH;
    pub fn FillRect(dc: HDC, rect: *const RECT, brush: HBRUSH) -> i32;
}
