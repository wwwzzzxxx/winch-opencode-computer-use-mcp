use std::sync::OnceLock;

use crate::ffi::*;
use crate::windows::wide;

const PNG_ENCODER: GUID = GUID {
    data1: 0x557C_F406,
    data2: 0x1A04,
    data3: 0x11D3,
    data4: [0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E],
};

static GDIP_TOKEN: OnceLock<usize> = OnceLock::new();

fn gdiplus_init() -> Result<&'static GdiplusApi, String> {
    let api = gdiplus().ok_or_else(|| "gdiplus.dll could not be loaded".to_string())?;
    if GDIP_TOKEN.get().is_none() {
        let mut token: usize = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        let status = unsafe { (api.startup)(&mut token, &input, std::ptr::null_mut()) };
        if status != 0 {
            return Err(format!("GdiplusStartup failed with status {status}"));
        }
        let _ = GDIP_TOKEN.set(token);
    }
    Ok(api)
}

pub struct Shot {
    pub png: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

/// True when the window actually paints something. `PrintWindow` failing or
/// producing a flat black/white frame means the content is gone (Electron apps
/// that tear down their web view when hidden), so `show` can tell a restored
/// window from an empty shell.
pub fn renders(hwnd: i64, w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return false;
        }
        let bw = w.min(256);
        let bh = h.min(256);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, bw, bh);
        if mem.is_null() || bmp.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return false;
        }
        let old = SelectObject(mem, bmp);
        let printed = PrintWindow(hwnd as HWND, mem, 2u32) != 0;
        let blank = printed && frame_is_flat(mem, bw, bh);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        printed && !blank
    }
}

/// Sample a grid of points: a frame is "flat" when every sample has the same
/// colour and that colour is pure black or pure white. This is how a torn-down
/// renderer looks, while a normal white page still has a dark header/sidebar.
unsafe fn frame_is_flat(dc: HDC, w: i32, h: i32) -> bool {
    if w < 4 || h < 4 {
        return false;
    }
    let pts = [
        (1, 1),
        (w / 2, 1),
        (w - 2, 1),
        (1, h / 2),
        (w / 2, h / 2),
        (w - 2, h / 2),
        (1, h - 2),
        (w / 2, h - 2),
        (w - 2, h - 2),
    ];
    let first = GetPixel(dc, pts[0].0, pts[0].1);
    if first != 0 && first != u32::MAX {
        return false;
    }
    pts.iter().all(|&(x, y)| GetPixel(dc, x, y) == first)
}

/// Capture with PrintWindow, falling back to a screen BitBlt when the window
/// renders black (common for DWM/D3D/WebView2 surfaces).
pub fn capture(hwnd: i64, x: i32, y: i32, w: i32, h: i32, max_width: Option<i32>) -> Result<Shot, String> {
    if w <= 0 || h <= 0 {
        return Err("window has no area".into());
    }
    let api = gdiplus_init()?;

    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("GetDC(screen) failed".into());
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if mem.is_null() || bmp.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("CreateCompatibleDC/Bitmap failed".into());
        }
        let old = SelectObject(mem, bmp);

        let printed = PrintWindow(hwnd as HWND, mem, 2u32) != 0;
        if !printed {
            // Never fall back to a screen grab here: for a window that paints
            // nothing it would silently return whatever is behind it (another
            // app) as if it were this window's content.
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("window did not render (PrintWindow failed); it may be blank or fully covered".into());
        }
        if frame_is_flat(mem, w, h) {
            // Flat frame: a real single-colour surface (D3D/WebView2) or a
            // torn-down window. Borrow the screen only when this window is
            // what is actually on top; otherwise the grab would return
            // another app's pixels.
            let on_top = crate::windows::window_at(x + w / 2, y + h / 2)
                .map(|u| u.id == crate::windows::root_owner(hwnd))
                .unwrap_or(false);
            if !on_top {
                SelectObject(mem, old);
                DeleteObject(bmp);
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(
                    "window paints a flat frame (blank or not composited); screenshot unavailable"
                        .into(),
                );
            }
            if BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY) == 0 {
                SelectObject(mem, old);
                DeleteObject(bmp);
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err("BitBlt failed".into());
            }
        }

        let mut use_bmp = bmp;
        let mut use_w = w;
        let mut use_h = h;
        let mut scaled: HBITMAP = std::ptr::null_mut();

        if let Some(mw) = max_width {
            if mw > 0 && w > mw {
                let nh = ((h as f64) * (mw as f64) / (w as f64)).round() as i32;
                scaled = CreateCompatibleBitmap(screen, mw, nh);
                if !scaled.is_null() {
                    let smem = CreateCompatibleDC(screen);
                    if !smem.is_null() {
                        let sold = SelectObject(smem, scaled);
                        let ok = StretchBlt(smem, 0, 0, mw, nh, mem, 0, 0, w, h, SRCCOPY);
                        SelectObject(smem, sold);
                        DeleteDC(smem);
                        if ok != 0 {
                            use_bmp = scaled;
                            use_w = mw;
                            use_h = nh;
                        }
                    }
                }
            }
        }

        let mut image: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = (api.create_from_hbitmap)(use_bmp, std::ptr::null_mut(), &mut image);
        if status != 0 || image.is_null() {
            SelectObject(mem, old);
            DeleteObject(bmp);
            if !scaled.is_null() {
                DeleteObject(scaled);
            }
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(format!("GdipCreateBitmapFromHBITMAP failed with status {status}"));
        }

        let file = std::env::temp_dir().join(format!("cu-{}-{}.png", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)));
        let wpath = wide(&file.to_string_lossy());
        let save = (api.save_to_file)(image, wpath.as_ptr(), &PNG_ENCODER, std::ptr::null());
        (api.dispose)(image);

        SelectObject(mem, old);
        DeleteObject(bmp);
        if !scaled.is_null() {
            DeleteObject(scaled);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);

        if save != 0 {
            return Err(format!("GdipSaveImageToFile failed with status {save}"));
        }
        let png = std::fs::read(&file).map_err(|e| format!("read screenshot: {e}"))?;
        let _ = std::fs::remove_file(&file);
        if png.is_empty() {
            return Err("screenshot is empty".into());
        }
        Ok(Shot {
            png,
            width: use_w,
            height: use_h,
        })
    }
}
