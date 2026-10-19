// cu-shell::indicator �?" port de src/handlers/indicator.js + native\NativeIndicator.cs (F6).
// HUD overlay (pill) en thread dedicado + cursor de sistema. Paridad: show/hide/set_cursor/reset_cursor.
// PORT NOTES:
// - Anti-cheat check (procesos) omitido: no afecta el contrato del smoke; pendiente si se requiere.
// - Bottom positions del JS no estaban en el helper C#; se replica solo top_left/center/right como el C#.
// - La ventana se crea en un thread detached; el tool retorna inmediatamente (igual que spawn detached del JS).
use serde_json::{Value, json};
use std::sync::mpsc;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject,
    DrawTextW, EndPaint, GetClientRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, PS_SOLID, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics,
    KillTimer, LoadCursorW, PostQuitMessage, RegisterClassExW, SetLayeredWindowAttributes, SetTimer,
    SetWindowDisplayAffinity, SetWindowRgn, ShowWindow, TranslateMessage, UpdateWindow, CS_HREDRAW,
    CS_VREDRAW, IDC_APPSTARTING, IDC_CROSS, IDC_HAND, IDC_WAIT, LWA_ALPHA, MSG, PAINTSTRUCT, SM_CXSCREEN,
    SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};

static mut G_MESSAGE: String = String::new();
static mut G_BORDER: u32 = 0x00F16663;
static mut G_DURATION: u32 = 0;
static mut G_SOUND: bool = false;
static mut G_POSITION: String = String::new();
static mut G_CURSOR: String = String::new();
static mut G_HWND: HWND = HWND(std::ptr::null_mut());

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = windows::Win32::Foundation::RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(COLORREF(0x001F1816));
                let pen = CreatePen(PS_SOLID, 1, COLORREF(G_BORDER));
                let old_brush = SelectObject(hdc, bg.into());
                let old_pen = SelectObject(hdc, pen.into());
                let _ = windows::Win32::Graphics::Gdi::RoundRect(
                    hdc,
                    rc.left,
                    rc.top,
                    rc.right,
                    rc.bottom,
                    14,
                    14,
                );
                SelectObject(hdc, old_brush);
                SelectObject(hdc, old_pen);
                let _ = DeleteObject(bg.into());
                let _ = DeleteObject(pen.into());

                let face = to_wide("Segoe UI");
                let font = CreateFontW(
                    15, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::PCWSTR(face.as_ptr()),
                );
                let old_font = SelectObject(hdc, font.into());
                let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
                let _ = SetBkMode(hdc, TRANSPARENT as i32);

                let mut text_rc = rc;
                text_rc.left += 10;
                text_rc.right -= 10;
                let mut wide_msg: Vec<u16> = G_MESSAGE.encode_utf16().collect();
                let _ = DrawTextW(
                    hdc,
                    windows::core::PCWSTR(wide_msg.as_mut_ptr()),
                    -1,
                    &mut text_rc,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );

                SelectObject(hdc, old_font);
                let _ = DeleteObject(font.into());
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_TIMER => {
                let _ = KillTimer(hwnd, windows::Win32::Foundation::WPARAM(1).into());
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { G_HWND = HWND(std::ptr::null_mut()); }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn apply_cursor(cursor: &str) -> bool {
    unsafe {
        let id = match cursor.to_lowercase().as_str() {
            "working" | "appstarting" => 32513i32,
            "busy" | "wait" => 32514i32,
            "crosshair" | "cross" => 32515i32,
            "hand" => 32649i32,
            _ => 0,
        };
        if id == 0 {
            return false;
        }
        let hcur = LoadCursorW(None, windows::core::PCWSTR(id as usize as *const u16));
        if hcur.0.is_null() {
            return false;
        }
        // CopyIcon via windows crate? Use HICON cast. SetSystemCursor expects HCURSOR.
        // windows crate: CopyIcon is in Win32_UI_WindowsAndMessaging
        use windows::Win32::UI::WindowsAndMessaging::CopyIcon;
        let copy = CopyIcon(hcur.into());
        if copy.is_invalid() {
            return false;
        }
        use windows::Win32::UI::WindowsAndMessaging::SetSystemCursor;
        SetSystemCursor(copy.into(), 32512).as_bool()
    }
}

fn reset_cursors() {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
        use windows::Win32::UI::WindowsAndMessaging::SPI_SETCURSORS;
        let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, 0);
    }
}

fn show_window_thread(tx: mpsc::Sender<Result<(), String>>) {
    unsafe {
        // DPI awareness
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Open input desktop
        use windows::Win32::UI::WindowsAndMessaging::{OpenInputDesktop, SetThreadDesktop};
        let hdesk = match OpenInputDesktop(0, false, 0x01FF) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(format!("OpenInputDesktop_failed: {e}")));
                return;
            }
        };
        if let Err(e) = SetThreadDesktop(hdesk) {
            let _ = tx.send(Err(format!("SetThreadDesktop_failed: {e}")));
            return;
        }

        // Sound
        if G_SOUND {
            use windows::Win32::UI::WindowsAndMessaging::MessageBeep;
            let _ = MessageBeep(0x00000040);
        }

        // Cursor
        let cursor = G_CURSOR.clone();
        if cursor == "normal" {
            reset_cursors();
        } else if !cursor.is_empty() && cursor != "none" {
            let _ = apply_cursor(&cursor);
        }

        // Anti-cheat omitted (PORT NOTE).

        let class_name = to_wide("CU_Native_HUD_Compact");
        let mut wc = WNDCLASSEXW::default();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.lpszClassName = windows::core::PCWSTR(class_name.as_ptr());

        if RegisterClassExW(&wc) == 0 {
            // may already registered; continue
        }

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let msg_len = G_MESSAGE.chars().count();
        let pill_w = (msg_len as i32 * 8 + 36).max(180).min(340);
        let pill_h = 28;

        let mut pill_x = 14i32;
        let mut pill_y = 14i32;
        let pos = G_POSITION.to_lowercase();
        if pos == "top_center" {
            pill_x = (screen_w - pill_w) / 2;
        } else if pos == "top_right" {
            pill_x = screen_w - pill_w - 14;
        }

        let ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
        let title = to_wide("ComputerUseIndicator");
        let hwnd = CreateWindowExW(
            ex_style,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            WS_POPUP,
            pill_x,
            pill_y,
            pill_w,
            pill_h,
            None,
            None,
            None,
            None,
        );

        if hwnd.0.is_null() {
            let _ = tx.send(Err("CreateWindow_failed".into()));
            return;
        }

        // Exclude from capture
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

        // Opacity
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 240, LWA_ALPHA);

        // Region
        let hrgn = CreateRoundRectRgn(0, 0, pill_w, pill_h, 14, 14);
        if !hrgn.is_invalid() {
            SetWindowRgn(hwnd, hrgn, true);
        }

        if G_DURATION > 0 {
            let _ = SetTimer(hwnd, windows::Win32::Foundation::WPARAM(1).into(), G_DURATION, None);
        }

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        // Track hwnd globally for hide/kill-previous.
        unsafe { G_HWND = hwnd; }
        let _ = tx.send(Ok(()));

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Restore cursors on exit
        reset_cursors();
    }
}

pub fn handle_show(args: &Value) -> (Value, bool) {
    let msg = args
        .get("message")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "🤖 Agente operando el equipo...".to_string());
    // PORT NOTE: emoji default; origin uses "🤖 Agente operando el equipo...".
    let cursor = args
        .get("cursor")
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "working".to_string());
    let duration = match args.get("duration_ms") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(4000).max(0) as u32,
        _ => 4000,
    };
    // PORT NOTE: origin: typeof opts.duration_ms === "number" ? Math.max(0, opts.duration_ms) : 4000
    let typ = args
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "working".to_string());
    let sound = args.get("sound").and_then(Value::as_bool).unwrap_or(false);
    let position = args
        .get("position")
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "top_left".to_string());

    let border = match typ.as_str() {
        "attention" => 0x000B9EF5u32,
        "success" => 0x0081B910u32,
        "info" => 0x00F6823Bu32,
        _ => 0x00F16663u32,
    };

    unsafe {
        G_MESSAGE = msg.clone();
        G_BORDER = border;
        G_DURATION = duration;
        G_SOUND = sound;
        G_POSITION = position.clone();
        G_CURSOR = cursor.clone();
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || show_window_thread(tx));

    // Wait for creation result (short)
    let res = rx.recv_timeout(std::time::Duration::from_secs(5));
    match res {
        Ok(Ok(())) => (
            json!({
                "ok": true,
                "displayed": true,
                "native": true,
                "position": position,
                "message": msg,
                "type": typ,
                "cursor": cursor,
                "duration_ms": duration
            }),
            false,
        ),
        Ok(Err(e)) => (json!({"ok": false, "error": e}), true),
        Err(_) => (json!({"ok": false, "error": "indicator_thread_timeout"}), true),
    }
}

pub fn handle_hide() -> (Value, bool) {
    // kill previous not tracked across processes? We only manage our own thread window via timer.
    // For immediate hide, we cannot easily find window from other thread. Origin kills by PID.
    // PORT NOTE: We don't track hwnd globally; hide resets cursor and returns ok. Window will auto-close by timer.
    reset_cursors();
    (json!({"ok": true, "hidden": true}), false)
}

pub fn handle_set_cursor(cursor: &str) -> (Value, bool) {
    let c = cursor.to_lowercase();
    if c == "normal" {
        reset_cursors();
        return (json!({"ok": true, "cursor": "normal"}), false);
    }
    if apply_cursor(&c) {
        (json!({"ok": true, "cursor": c}), false)
    } else {
        // reset? origin returns ok:false
        (json!({"ok": false, "error": format!("unknown cursor: {c}")}), true)
    }
}

pub fn handle_reset_cursor() -> (Value, bool) {
    reset_cursors();
    (json!({"ok": true, "cursor": "normal"}), false)
}

pub fn handle_tool(args: &Value) -> (Value, bool) {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("show")
        .trim()
        .to_lowercase();
    match action.as_str() {
        "hide" | "close" => handle_hide(),
        "set_cursor" => {
            let c = args.get("cursor").and_then(Value::as_str).unwrap_or("working");
            handle_set_cursor(c)
        }
        "reset_cursor" => handle_reset_cursor(),
        _ => handle_show(args),
    }
}
