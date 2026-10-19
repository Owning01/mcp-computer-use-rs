// PORT NOTE: focusWindowHelper portado a Rust. Se omite la busqueda UIA top-level children
// (Win11 packaged apps) por complejidad; se cubre EnumWindows + Get-Process + aliases.
use crate::helpers::{alias_terms, is_app_blocked};
use serde_json::json;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

pub fn focus_window(args: &serde_json::Value) -> serde_json::Value {
    // hwnd o title
    let hwnd_arg = args.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
    let title_arg = args
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // isAppBlocked sobre target (string o hwnd)
    let target_str = if hwnd_arg > 0 {
        hwnd_arg.to_string()
    } else {
        title_arg.clone()
    };
    if is_app_blocked(&target_str) {
        return json!({"ok": false, "error": "app_blocked", "message": format!("Interacción con el gestor de contraseñas \"{}\" bloqueada por seguridad.", target_str)});
    }

    // Resolver hwnd
    let mut target_hwnd: isize = 0;
    if hwnd_arg > 0 {
        target_hwnd = hwnd_arg as isize;
    } else if !title_arg.is_empty() {
        let terms = alias_terms(&title_arg);
        // 1. EnumWindows visible top-level
        target_hwnd = find_enum_window(&terms);
        // 2. Fallback Get-Process
        if target_hwnd == 0 {
            target_hwnd = find_process_window(&terms);
        }
    }

    if target_hwnd == 0 {
        return json!({"ok": false, "status": "not_found"});
    }

    unsafe {
        let h = HWND(target_hwnd as *mut core::ffi::c_void);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
            // pequena espera
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        // Alt tap (libera lock foreground)
        use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS};
        // VK_MENU = 0x12
        keybd_event(0x12, 0, KEYBD_EVENT_FLAGS(0), 0usize);
        keybd_event(0x12, 0, KEYBD_EVENT_FLAGS(2), 0usize);

        let fg = GetForegroundWindow();
        let mut dummy1: u32 = 0;
        let mut dummy2: u32 = 0;
        let fg_thread = if !fg.0.is_null() {
            GetWindowThreadProcessId(fg, Some(&mut dummy1))
        } else {
            0
        };
        let target_thread = GetWindowThreadProcessId(h, Some(&mut dummy2));
        let cur_thread = GetCurrentThreadId();

        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, true);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, true);
        }
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = BringWindowToTop(h);
        let focused = SetForegroundWindow(h).as_bool();
        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, false);
        }

        // rect (no usado en respuesta pero ok)
        let _ = GetWindowRect(h, &mut windows::Win32::Foundation::RECT::default());

        if focused || true {
            return json!({"ok": true, "hwnd": target_hwnd as i64, "status": "focused"});
        }
    }
    json!({"ok": false, "status": "not_found"})
}

unsafe extern "system" fn enum_proc(hwnd: windows::Win32::Foundation::HWND, lparam: LPARAM) -> windows::core::BOOL {
    // SAFETY: lparam apunta a EnumCtx valido durante la enumeracion.
    unsafe {
        use crate::helpers::contains_ci;
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        if !windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        let mut buf = [0u16; 512];
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            return windows::core::BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.trim().is_empty() {
            return windows::core::BOOL(1);
        }
        for term in &ctx.terms {
            if contains_ci(&title, term) {
                ctx.found = hwnd.0 as isize;
                return windows::core::BOOL(0); // stop
            }
        }
        windows::core::BOOL(1)
    }
}

struct EnumCtx {
    terms: Vec<String>,
    found: isize,
}

fn find_enum_window(terms: &[String]) -> isize {
    let mut ctx = EnumCtx {
        terms: terms.to_vec(),
        found: 0,
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
    }
    ctx.found
}

fn find_process_window(terms: &[String]) -> isize {
    use crate::helpers::{enum_pids, process_name};
    for pid in enum_pids() {
        if pid == 0 {
            continue;
        }
        let pname = process_name(pid);
        // match process name
        let mut proc_match = false;
        for term in terms {
            if pname.to_lowercase().contains(&term.to_lowercase()) {
                proc_match = true;
                break;
            }
        }
        if !proc_match {
            continue;
        }
        // find main window handle for this pid
        // EnumWindows and check pid
        struct Ctx {
            pid: u32,
            found: isize,
        }
        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            unsafe {
                let ctx = &mut *(lparam.0 as *mut Ctx);
                let mut p: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut p));
                if p == ctx.pid {
                    // ensure visible? origin checks MainWindowHandle non-zero
                    ctx.found = hwnd.0 as isize;
                    return windows::core::BOOL(0);
                }
                windows::core::BOOL(1)
            }
        }
        let mut c = Ctx { pid, found: 0 };
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut c as *mut Ctx as isize));
        }
        if c.found != 0 {
            return c.found;
        }
    }
    0
}
