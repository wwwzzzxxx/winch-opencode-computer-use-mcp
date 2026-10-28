//! Clipboard read/write through the Win32 clipboard, so a copy action can be
//! checked instead of assumed: click the copy icon, then `{op:"clipboard"}`.

use std::ffi::c_void;

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 2;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(owner: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn SetClipboardData(format: u32, mem: isize) -> isize;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalLock(mem: isize) -> *mut c_void;
    fn GlobalUnlock(mem: isize) -> i32;
    fn GlobalFree(mem: isize) -> isize;
}

/// The clipboard is a single machine-wide resource and other apps hold it for
/// a few milliseconds at a time; retry briefly instead of failing the step.
fn open() -> Result<(), String> {
    for _ in 0..10 {
        if unsafe { OpenClipboard(0) } != 0 {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err("OpenClipboard failed: another process is holding the clipboard".into())
}

pub fn read() -> Result<String, String> {
    open()?;
    let mut out = String::new();
    unsafe {
        let mem = GetClipboardData(CF_UNICODETEXT);
        if mem == 0 {
            CloseClipboard();
            return Ok(out); // no text on the clipboard (empty, or an image/file list)
        }
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return Err("GlobalLock on the clipboard text failed".into());
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        out = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        GlobalUnlock(mem);
        CloseClipboard();
    }
    Ok(out)
}

pub fn write(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if mem == 0 {
            return Err("GlobalAlloc for the clipboard failed".into());
        }
        let dst = GlobalLock(mem) as *mut u16;
        if dst.is_null() {
            GlobalFree(mem);
            return Err("GlobalLock for the clipboard failed".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(mem);

        if let Err(e) = open() {
            GlobalFree(mem);
            return Err(e);
        }
        EmptyClipboard();
        if SetClipboardData(CF_UNICODETEXT, mem) == 0 {
            CloseClipboard();
            GlobalFree(mem);
            return Err("SetClipboardData failed".into());
        }
        CloseClipboard(); // ownership of `mem` moved to the clipboard
    }
    Ok(())
}
