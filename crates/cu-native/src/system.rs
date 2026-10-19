// cu-native::system — active_window + clipboard (F6, repo-migrate).
// Contratos: GetActiveWindowJson() de index.js (NATIVE_WIN32_HEADER, DPI PMv2) y
// computer_clipboard (get: PS Get-Clipboard trimeado; set: Set-Clipboard).
use serde_json::{Value, json};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, RECT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetLastActivePopup, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
};

unsafe fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

// Mismo shape/orden que GetActiveWindowJson(): hwnd, owner_hwnd, title, is_modal, modal_title,
// x, y, w, width, h, height, pid, active.
pub fn active_window() -> Value {
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return json!({"hwnd":0,"title":"","active":false,"is_modal":false});
        }
        let pop = GetLastActivePopup(fg);
        let (target, is_modal, modal_title) = if !pop.0.is_null() && pop != fg {
            (pop, true, window_title(pop))
        } else {
            (fg, false, String::new())
        };
        let title = window_title(target);
        let mut r = RECT::default();
        let _ = GetWindowRect(target, &mut r);
        let mut pid: u32 = 0;
        let _ = GetWindowThreadProcessId(target, Some(&mut pid));
        let w = r.right - r.left;
        let h = r.bottom - r.top;
        json!({
            "hwnd": target.0 as isize as i64,
            "owner_hwnd": fg.0 as isize as i64,
            "title": title,
            "is_modal": is_modal,
            "modal_title": modal_title,
            "x": r.left,
            "y": r.top,
            "w": w,
            "width": w,
            "h": h,
            "height": h,
            "pid": pid,
            "active": true
        })
    }
}

pub fn clipboard_get() -> Result<String, String> {
    unsafe {
        OpenClipboard(None).map_err(|e| format!("open_clipboard_failed: {e}"))?;
        let res = (|| -> Result<String, String> {
            match GetClipboardData(CF_UNICODETEXT.0 as u32) {
                Ok(handle) => {
                    let h = HGLOBAL(handle.0);
                    let ptr = GlobalLock(h) as *const u16;
                    if ptr.is_null() {
                        return Ok(String::new());
                    }
                    let mut len = 0usize;
                    while *ptr.add(len) != 0 {
                        len += 1;
                    }
                    let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                    let _ = GlobalUnlock(h);
                    Ok(s)
                }
                Err(_) => Ok(String::new()),
            }
        })();
        let _ = CloseClipboard();
        res
    }
}

pub fn clipboard_set(text: &str) -> Result<(), String> {
    unsafe {
        OpenClipboard(None).map_err(|e| format!("open_clipboard_failed: {e}"))?;
        let res = (|| -> Result<(), String> {
            EmptyClipboard().map_err(|e| format!("empty_clipboard_failed: {e}"))?;
            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = utf16.len() * 2;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes)
                .map_err(|e| format!("global_alloc_failed: {e}"))?;
            let ptr = GlobalLock(h) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(h));
                return Err("global_lock_failed".into());
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
            let _ = GlobalUnlock(h);
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_err() {
                let _ = GlobalFree(Some(h));
                return Err("set_clipboard_data_failed".into());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        res
    }
}
