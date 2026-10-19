// cu-input — input nativo (SendInput/keybd_event/mouse_event). Reglas: migration/RULEBOOK.md.
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
use base64::Engine;
use cu_core::out::ToolOut;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::System::Threading::AttachThreadInput;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}

// ---------------------------------------------------------------------------
// Helpers de semántica JS (int/clamp).
// ---------------------------------------------------------------------------
fn int_arg(v: Option<&Value>, min: i64, max: i64, def: i64) -> i64 {
    let n = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    };
    let n = match n {
        Some(f) if f.is_finite() => f.trunc() as i64,
        _ => def,
    };
    n.clamp(min, max)
}

fn get_vk(k: &str) -> u8 {
    let kl = k.trim().to_lowercase();
    match kl.as_str() {
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" | "bs" => 0x08,
        "delete" | "del" => 0x2E,
        "space" => 0x20,
        "up" | "arrowup" => 0x26,
        "down" | "arrowdown" => 0x28,
        "left" | "arrowleft" => 0x25,
        "right" | "arrowright" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "meta" | "win" | "super" | "cmd" => 0x5B,
        _ => {
            if kl.len() == 1 {
                let c = kl.chars().next().unwrap();
                if c.is_ascii_lowercase() {
                    return 0x41 + (c as u8 - b'a');
                }
                if c.is_ascii_digit() {
                    return 0x30 + (c as u8 - b'0');
                }
            }
            0
        }
    }
}

fn parse_hotkey(combo: &str) -> (String, Vec<String>) {
    let parts: Vec<String> = combo
        .to_lowercase()
        .split('+')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut mods = Vec::new();
    let mut key = String::new();
    for p in parts {
        if p == "ctrl" || p == "control" {
            mods.push("ctrl".to_string());
        } else if p == "shift" {
            mods.push("shift".to_string());
        } else if p == "alt" {
            mods.push("alt".to_string());
        } else if p == "win" || p == "meta" || p == "super" || p == "cmd" {
            mods.push("meta".to_string());
        } else {
            key = p;
        }
    }
    (key, mods)
}

fn native_key_action(key: &str, action: &str, mods: &[String]) {
    let mod_vks: Vec<u8> = mods
        .iter()
        .map(|m| get_vk(m))
        .filter(|&v| v != 0)
        .collect();
    let main_vk = get_vk(key);
    unsafe {
        for &m in &mod_vks {
            keybd_event(m, 0, KEYBD_EVENT_FLAGS(0), 0);
        }
        match action {
            "down" => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                }
            }
            // PORT NOTE: en "up" el origen libera los modificadores en orden (no invertido).
            "up" => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYEVENTF_KEYUP, 0);
                }
                for &m in &mod_vks {
                    keybd_event(m, 0, KEYEVENTF_KEYUP, 0);
                }
            }
            _ => {
                if main_vk != 0 {
                    keybd_event(main_vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                    thread::sleep(Duration::from_millis(20));
                    keybd_event(main_vk, 0, KEYEVENTF_KEYUP, 0);
                }
                let mut rev = mod_vks.clone();
                rev.reverse();
                for &m in &rev {
                    keybd_event(m, 0, KEYEVENTF_KEYUP, 0);
                }
            }
        }
    }
    clear_failsafe_modifiers();
}

// PORT NOTE: origin `Clear-FailsafeModifiers` (index.js ~L395): tras cada nativeKeyAction libera
// Shift/Ctrl/Alt/Win que estén pulsados según GetAsyncKeyState. Replicado tal cual.
fn clear_failsafe_modifiers() {
    for vk in [0x10u8, 0x11, 0x12, 0x5B, 0x5C] {
        let state = unsafe { GetAsyncKeyState(vk as i32) };
        if (state as u16) & 0x8000 != 0 {
            unsafe { keybd_event(vk, 0, KEYEVENTF_KEYUP, 0) };
        }
    }
}

// ---------------------------------------------------------------------------
// Foco de ventana (versión simplificada del helper del origen).
// ---------------------------------------------------------------------------
unsafe fn find_window_by_title(needle: &str) -> HWND {
    let needle_l = needle.to_lowercase();
    struct Ctx {
        needle: String,
        found: HWND,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            if !IsWindowVisible(hwnd).as_bool() {
                return windows::core::BOOL(1);
            }
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
                if !ctx.needle.is_empty() && title.contains(&ctx.needle) {
                    ctx.found = hwnd;
                    return windows::core::BOOL(0);
                }
            }
            windows::core::BOOL(1)
        }
    }
    let mut ctx = Ctx {
        needle: needle_l,
        found: HWND(std::ptr::null_mut()),
    };
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize)); }
    ctx.found
}

unsafe fn focus_window(target: &Value) {
    // Resolver hwnd.
    let mut hwnd_val: i64 = 0;
    if let Some(n) = target.as_i64() {
        hwnd_val = n;
    } else if let Some(s) = target.as_str() {
        let t = s.trim();
        if let Ok(n) = t.parse::<i64>() {
            hwnd_val = n;
        }
    }
    let mut target_hwnd = if hwnd_val > 0 {
        HWND(hwnd_val as *mut _)
    } else {
        HWND(std::ptr::null_mut())
    };
    if target_hwnd.0.is_null() {
        if let Some(s) = target.as_str() {
            target_hwnd = unsafe { find_window_by_title(s) };
        }
    }
    if target_hwnd.0.is_null() {
        return;
    }
    // Restaurar si está minimizada.
    unsafe {
        if IsIconic(target_hwnd).as_bool() {
            let _ = ShowWindow(target_hwnd, SW_RESTORE);
            thread::sleep(Duration::from_millis(40));
        }
    }
    let mut pid: u32 = 0;
    let fg = unsafe { GetForegroundWindow() };
    let fg_thread = if !fg.0.is_null() {
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) }
    } else {
        0
    };
    let target_thread = unsafe { GetWindowThreadProcessId(target_hwnd, Some(&mut pid)) };
    let cur_thread = unsafe { GetCurrentThreadId() };
    unsafe {
        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, true);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, true);
        }
        let _ = ShowWindow(target_hwnd, SW_RESTORE);
        let _ = BringWindowToTop(target_hwnd);
        let _ = SetForegroundWindow(target_hwnd);
        if fg_thread != 0 && cur_thread != fg_thread {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
        if target_thread != 0 && cur_thread != target_thread {
            let _ = AttachThreadInput(cur_thread, target_thread, false);
        }
    }
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_hotkey"
            | "computer_key"
            | "computer_move"
            | "computer_copy"
            | "computer_paste"
            | "computer_bulk_input"
            | "computer_drag_scroll"
            | "computer_click"
            | "computer_double_click"
            | "computer_human_click"
            | "computer_drag"
            | "computer_scroll"
            | "computer_type"
            | "computer_handle_file_dialog"
    )
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name {
        "computer_hotkey" => handle_hotkey(args),
        "computer_key" => handle_key(args),
        "computer_move" => handle_move(args),
        "computer_copy" => handle_copy(args),
        "computer_paste" => handle_paste(args),
        "computer_bulk_input" => handle_bulk_input(args),
        "computer_drag_scroll" => handle_drag_scroll(args),
        "computer_click" => handle_click(args, false),
        "computer_double_click" => handle_click(args, true),
        "computer_human_click" => handle_human_click(args),
        "computer_drag" => handle_drag(args),
        "computer_scroll" => handle_scroll(args),
        "computer_type" => handle_type(args),
        "computer_handle_file_dialog" => handle_file_dialog(args),
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }));
    match result {
        Ok(out) => out,
        Err(_) => ToolOut::Json(json!({"ok": false, "error": "panic"}), true),
    }
}

fn handle_hotkey(args: &Value) -> ToolOut {
    let combo = args
        .get("combo")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default()
        .trim()
        .to_string();
    if combo.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "combo parameter is required (e.g. 'ctrl+c', 'win+r')"}), true);
    }
    let (key, mods) = parse_hotkey(&combo);
    if key.is_empty() && mods.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": format!("Invalid hotkey combo: {}", combo)}), true);
    }
    let mods_ref: Vec<String> = mods.clone();
    native_key_action(&key, "tap", &mods_ref);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({"ok": true, "hotkey": combo, "key": key, "mods": mods, "epoch": epoch}),
        false,
    )
}

fn handle_key(args: &Value) -> ToolOut {
    let key = args
        .get("key")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default();
    let action = args
        .get("action")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "tap".to_string());
    let action = if action.is_empty() { "tap".to_string() } else { action };
    let mods: Vec<String> = match args.get("mods") {
        Some(Value::Array(a)) => a.iter().map(cu_core::js::js_string_of).collect(),
        Some(v) => vec![cu_core::js::js_string_of(v)],
        None => Vec::new(),
    };
    native_key_action(&key, &action, &mods);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "action": "key", "epoch": epoch}), false)
}

fn handle_move(args: &Value) -> ToolOut {
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "action": "move", "epoch": epoch}), false)
}

fn handle_copy(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            unsafe { focus_window(tw) };
        }
    }
    native_key_action("c", "tap", &["ctrl".to_string()]);
    thread::sleep(Duration::from_millis(60));
    let text = cu_native::system::clipboard_get().unwrap_or_default().trim().to_string();
    ToolOut::Json(json!({"ok": true, "text": text}), false)
}

fn handle_paste(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            // app_blocked check omitido (no disponible en Rust); foco directo.
            unsafe { focus_window(tw) };
        }
    }
    let mut text = args
        .get("text")
        .map(cu_core::js::js_string_of)
        .unwrap_or_default();
    let is_secret = args.get("secret_b64").is_some();
    if let Some(b64) = args.get("secret_b64").and_then(Value::as_str) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            text = String::from_utf8_lossy(&bytes).to_string();
        }
    }
    let press_enter = args
        .get("press_enter")
        .map(|v| cu_core::js::js_truthy(v))
        .unwrap_or(false);

    // nativeSafePaste
    let prev = cu_native::system::clipboard_get().unwrap_or_default();
    let _ = cu_native::system::clipboard_set(&text);
    native_key_action("v", "tap", &["ctrl".to_string()]);
    if press_enter {
        thread::sleep(Duration::from_millis(40));
        unsafe {
            keybd_event(0x0D, 0, KEYBD_EVENT_FLAGS(0), 0);
            thread::sleep(Duration::from_millis(20));
            keybd_event(0x0D, 0, KEYEVENTF_KEYUP, 0);
        }
    }
    thread::sleep(Duration::from_millis(60));
    // restaurar clipboard previo
    if !prev.is_empty() {
        let _ = cu_native::system::clipboard_set(&prev);
    } else {
        let _ = cu_native::system::clipboard_set("");
    }
    let _ = is_secret;
    let epoch = cu_core::epoch::current();
    let payload = json!({
        "ok": true,
        "action": "paste",
        "epoch": epoch,
        "clipboard_restored": true
    });
    ToolOut::Json(payload, false)
}

fn handle_bulk_input(args: &Value) -> ToolOut {
    let rows = args.get("rows").cloned().unwrap_or(Value::Array(Vec::new()));
    let rows_arr = match rows {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    if rows_arr.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "rows parameter must be a non-empty 2D array"}), true);
    }
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() {
            unsafe { focus_window(tw) };
        }
    }
    let target_x = int_arg(args.get("target_x"), -100000, 100000, 0) as i32;
    let target_y = int_arg(args.get("target_y"), -100000, 100000, 0) as i32;
    // PORT NOTE: origin clic solo si x>0 && y>0 (PowerShell -gt 0), con down/up y esperas 40/80.
    if target_x > 0 && target_y > 0 {
        unsafe {
            let _ = SetCursorPos(target_x, target_y);
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(80));
        }
    }

    // Construir TSV como el origen: celdas [string] (null => ""), filas unidas por [Environment]::NewLine.
    let mut tsv_lines: Vec<String> = Vec::new();
    let mut total_cells: i64 = 0;
    for row in &rows_arr {
        let cells: Vec<String> = match row {
            Value::Array(a) => a
                .iter()
                .map(|c| match c {
                    Value::Null => String::new(),
                    other => cu_core::js::js_string_of(other),
                })
                .collect(),
            _ => vec![cu_core::js::js_string_of(row)],
        };
        total_cells += cells.len() as i64;
        tsv_lines.push(cells.join("\t"));
    }
    let tsv_text = tsv_lines.join("\r\n");

    let _ = cu_native::system::clipboard_set(&tsv_text);
    thread::sleep(Duration::from_millis(40));
    native_key_action("v", "tap", &["ctrl".to_string()]);

    let row_count = rows_arr.len() as i64;
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(
        json!({
            "ok": true,
            "rows_injected": row_count,
            "total_cells": total_cells,
            "method": "clipboard_tsv_grid",
            "epoch": epoch
        }),
        false,
    )
}

fn handle_drag_scroll(args: &Value) -> ToolOut {
    // Parse payload like origin handle.
    let mode = args
        .get("mode")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "rubber_band".to_string())
        .to_lowercase();
    let mode = if mode.is_empty() { "rubber_band".to_string() } else { mode };

    let from_x = if args.get("from_x").is_none() { 0 } else { int_arg(args.get("from_x"), -1_000_000, 1_000_000, 0) as i32 };
    let from_y = if args.get("from_y").is_none() { 0 } else { int_arg(args.get("from_y"), -1_000_000, 1_000_000, 0) as i32 };
    let to_x = if args.get("to_x").is_none() { 0 } else { int_arg(args.get("to_x"), -1_000_000, 1_000_000, 0) as i32 };
    let to_y = if args.get("to_y").is_none() { 0 } else { int_arg(args.get("to_y"), -1_000_000, 1_000_000, 0) as i32 };

    let button = args
        .get("button")
        .map(cu_core::js::js_string_of)
        .unwrap_or_else(|| "left".to_string())
        .to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };

    let modifiers: Vec<String> = match args.get("modifiers") {
        Some(Value::Array(a)) => a.iter().map(cu_core::js::js_string_of).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(v) if cu_core::js::js_truthy(v) => vec![cu_core::js::js_string_of(v)],
        Some(_) => Vec::new(),
    };

    // PORT NOTE: origin `Number(x) || default` y luego C#: steps/delay <=0 => default; sin clamp.
    let steps = match args.get("steps").and_then(Value::as_i64) {
        Some(n) if n > 0 => n as i32,
        _ => 14,
    };
    let delay_ms = match args.get("delay_ms").and_then(Value::as_i64) {
        Some(n) if n > 0 => n as i32,
        _ => 15,
    };

    // PORT NOTE: origin usa `args.mode === "drag_scroll"` crudo (sin lowercase) para el default de hold.
    let hold_default: i64 = if args.get("mode").and_then(Value::as_str) == Some("drag_scroll") { 800 } else { 0 };
    let hold_duration_ms = args.get("hold_duration_ms").and_then(Value::as_i64).unwrap_or(hold_default) as i32;

    let scroll_pulses = args.get("scroll_pulses").and_then(Value::as_i64).unwrap_or(0) as i32;
    let scroll_delta = match args.get("scroll_delta").and_then(Value::as_i64) {
        Some(n) if n != 0 => n as i32,
        _ => -120,
    };
    // PORT NOTE: origin `args.release !== false` (estricto).
    let release = args.get("release") != Some(&Value::Bool(false));

    // Ejecutar gesto.
    let mut modifiers_applied: Vec<String> = Vec::new();
    let mut pressed_keys: Vec<u8> = Vec::new();
    let mut pulses_sent: i64 = 0;

    let (down_flag, up_flag) = if button == "right" {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else if button == "middle" {
        (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };

    unsafe {
        // press modifiers
        for m in &modifiers {
            // PORT NOTE: C# GetVk hace ToLower().Trim(); modifiers_applied guarda el texto original.
            let vk = match m.trim().to_lowercase().as_str() {
                "ctrl" | "control" => 0x11u8,
                "shift" => 0x10u8,
                "alt" => 0x12u8,
                _ => 0u8,
            };
            if vk != 0 && !pressed_keys.contains(&vk) {
                keybd_event(vk, 0, KEYBD_EVENT_FLAGS(0), 0);
                pressed_keys.push(vk);
                modifiers_applied.push(m.clone());
            }
        }
        if !pressed_keys.is_empty() {
            thread::sleep(Duration::from_millis(30));
        }

        let _ = SetCursorPos(from_x, from_y);
        mouse_event(down_flag, 0, 0, 0, 0);
        thread::sleep(Duration::from_millis(40));

        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let smooth = t * t * (3.0 - 2.0 * t);
            // PORT NOTE: C# Math.Round (mitad al par).
            let cur_x = (from_x as f64 + (to_x - from_x) as f64 * smooth).round_ties_even() as i32;
            let cur_y = (from_y as f64 + (to_y - from_y) as f64 * smooth).round_ties_even() as i32;
            let _ = SetCursorPos(cur_x, cur_y);
            thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        let _ = SetCursorPos(to_x, to_y);

        if (mode == "drag_scroll" || mode == "scroll") && hold_duration_ms > 0 {
            let total_hold = hold_duration_ms;
            let interval_ms = 60i32;
            let iterations = (total_hold / interval_ms).max(1);
            let pulse_step = if scroll_pulses > 0 {
                (iterations / scroll_pulses).max(1)
            } else {
                0
            };
            for i in 0..iterations {
                let jitter = if i % 2 == 0 { 1 } else { -1 };
                let _ = SetCursorPos(to_x + jitter, to_y);
                thread::sleep(Duration::from_millis((interval_ms / 2) as u64));
                let _ = SetCursorPos(to_x, to_y);
                thread::sleep(Duration::from_millis((interval_ms / 2) as u64));
                if scroll_pulses > 0 && pulse_step > 0 && (i % pulse_step == 0) && pulses_sent < scroll_pulses as i64 {
                    let delta = if scroll_delta != 0 { scroll_delta } else { -120 };
                    mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta, 0);
                    pulses_sent += 1;
                }
            }
        }

        if release {
            thread::sleep(Duration::from_millis(30));
            mouse_event(up_flag, 0, 0, 0, 0);
        }

        // release modifiers
        for vk in &pressed_keys {
            keybd_event(*vk, 0, KEYEVENTF_KEYUP, 0);
        }
    }

    let mut from_obj = serde_json::Map::new();
    from_obj.insert("x".to_string(), json!(from_x));
    from_obj.insert("y".to_string(), json!(from_y));
    let mut to_obj = serde_json::Map::new();
    to_obj.insert("x".to_string(), json!(to_x));
    to_obj.insert("y".to_string(), json!(to_y));

    let mut res = serde_json::Map::new();
    res.insert("ok".to_string(), json!(true));
    res.insert("mode".to_string(), json!(mode));
    res.insert("from".to_string(), Value::Object(from_obj));
    res.insert("to".to_string(), Value::Object(to_obj));
    res.insert("modifiers_applied".to_string(), json!(modifiers_applied));
    res.insert("steps".to_string(), json!(steps));
    res.insert("pulses_sent".to_string(), json!(pulses_sent));
    // PORT NOTE: ConvertTo-Json de PowerShell incluye "error": null al final (propiedad sin valor).
    res.insert("error".to_string(), Value::Null);

    // PORT NOTE: index.js hace currentEpoch++ antes de dragScroll.handle (el epoch no va en el payload).
    cu_core::epoch::bump();
    ToolOut::Json(Value::Object(res), false)
}

// Presencia de clave en el sentido del origen (`args.x !== undefined`): null cuenta como presente.
fn has_arg(args: &Value, key: &str) -> bool {
    args.get(key).is_some()
}

// Enteros JSON dentro del rango de clamp del origen (sin redondeo ni coerción de strings).
fn int_or_absent(args: &Value, key: &str) -> bool {
    match args.get(key) {
        None => true,
        Some(v) => v.as_i64().map(|n| (-100000..=100000).contains(&n)).unwrap_or(false),
    }
}

fn int_present(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_i64).map(|n| (-100000..=100000).contains(&n)).unwrap_or(false)
}

// Flags booleanos: el origen usa `=== true` en human_like/human_jitter; solo acepto bool o ausente.
fn bool_or_absent(args: &Value, key: &str) -> bool {
    match args.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::Bool(_)) => true,
        _ => false,
    }
}

fn truthy(args: &Value, key: &str) -> bool {
    cu_core::js::flag_truthy(args.get(key))
}

// PORT NOTE (P0, opción A): el origen resuelve behavior "auto" (ausente incluido) según ventana,
// patrones UIA y COMPUTER_USE_BEHAVIOR. No se porta esa política: solo se acepta un behavior
// explícito "foreground" o "humanized" (resolveBehavior devuelve ese modo sin consultar ventana).
fn behavior_explicit(args: &Value) -> bool {
    matches!(args.get("behavior").and_then(Value::as_str), Some("foreground") | Some("humanized"))
}

// PORT NOTE: origin `HARNESS_MODE = (COMPUTER_USE_MODE || "native").toLowerCase()`; en harness/auto
// `api()` intenta el shell HTTP antes del camino nativo => proxy.
fn native_mode_only() -> bool {
    let m = std::env::var("COMPUTER_USE_MODE").unwrap_or_default().to_lowercase();
    m.is_empty() || m == "native"
}

// PORT NOTE: origin `button` pasa por mouseButton() (exacto: right/middle, resto left).
fn button_ok(args: &Value) -> bool {
    match args.get("button") {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => matches!(s.as_str(), "left" | "right" | "middle"),
        _ => false,
    }
}

// PORT NOTE: focusWindowHelper lanza app_blocked para gestores de contraseña (BLOCKED_APP_FRAGMENTS
// de utils/error_recovery.js). Si el target cae ahí, el origen devuelve error => proxy.
fn target_blocked(args: &Value) -> bool {
    const BLOCKED: [&str; 7] = ["1password", "bitwarden", "dashlane", "lastpass", "nordpass", "proton pass", "keepass"];
    match args.get("target_window").and_then(Value::as_str) {
        Some(t) => {
            let lower = t.to_lowercase();
            BLOCKED.iter().any(|f| lower.contains(f))
        }
        None => false,
    }
}

fn no_snapshot(args: &Value) -> bool {
    !truthy(args, "snapshot_after")
}

// Texto que SendInput unicode reproduce igual que SendKeys: sin controles y sin surrogates.
fn plain_text_ok(s: &str) -> bool {
    s.chars().all(|c| (c as u32) >= 0x20 && (c as u32) != 0x7f && (c as u32) < 0x10000)
}

// Celdas bulk: el origen hace [string]$c en PowerShell; acepto solo string, entero o null.
fn cell_ok(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Null) || v.as_i64().is_some()
}

pub fn accepts(name: &str, args: &Value) -> bool {
    match name {
        "computer_hotkey" => match args.get("combo") {
            Some(Value::String(s)) => {
                let (key, mods) = parse_hotkey(s.trim());
                !key.is_empty() || !mods.is_empty()
            }
            _ => false,
        },
        "computer_key" => {
            let key_ok = matches!(args.get("key"), Some(Value::String(s)) if !s.is_empty());
            let action_ok = match args.get("action") {
                None | Some(Value::Null) => true,
                Some(Value::String(s)) => matches!(s.as_str(), "tap" | "down" | "up"),
                _ => false,
            };
            let mods_ok = match args.get("mods") {
                None | Some(Value::Null) => true,
                Some(Value::Array(a)) => a.iter().all(|m| m.is_string()),
                _ => false,
            };
            native_mode_only() && key_ok && action_ok && mods_ok
        }
        "computer_move" => native_mode_only() && int_or_absent(args, "x") && int_or_absent(args, "y"),
        "computer_copy" => !target_blocked(args),
        "computer_paste" => !target_blocked(args) && no_snapshot(args),
        "computer_bulk_input" => {
            let rows_ok = match args.get("rows") {
                Some(Value::Array(a)) => !a.is_empty() && a.iter().all(|r| match r {
                    Value::Array(cells) => cells.iter().all(cell_ok),
                    _ => false,
                }),
                _ => false,
            };
            if !rows_ok || target_blocked(args) { return false; }
            if !(int_or_absent(args, "target_x") && int_or_absent(args, "target_y")) { return false; }
            // origin: `!targetX && !targetY && target_text` resuelve por UIA => proxy.
            if has_arg(args, "target_text") {
                let tx = args.get("target_x").and_then(Value::as_i64).unwrap_or(0);
                let ty = args.get("target_y").and_then(Value::as_i64).unwrap_or(0);
                if tx == 0 && ty == 0 { return false; }
            }
            true
        }
        "computer_drag_scroll" => {
            let mode_ok = match args.get("mode") {
                None | Some(Value::Null) => true,
                Some(Value::String(_)) => true,
                _ => false,
            };
            let mods_ok = match args.get("modifiers") {
                None | Some(Value::Null) => true,
                Some(Value::Array(a)) => a.iter().all(|m| m.is_string()),
                Some(Value::String(_)) => true,
                _ => false,
            };
            let nums_ok = ["from_x", "from_y", "to_x", "to_y", "steps", "delay_ms", "hold_duration_ms", "scroll_pulses", "scroll_delta"]
                .iter()
                .all(|k| int_or_absent(args, k));
            let release_ok = match args.get("release") {
                None | Some(Value::Null) | Some(Value::Bool(_)) => true,
                _ => false,
            };
            let button_str_ok = match args.get("button") {
                None | Some(Value::Null) => true,
                Some(Value::String(_)) => true,
                _ => false,
            };
            mode_ok && mods_ok && nums_ok && release_ok && button_str_ok && native_mode_only()
        }
        "computer_click" | "computer_double_click" => {
            if !int_present(args, "x") || !int_present(args, "y") { return false; }
            if !button_ok(args) || !bool_or_absent(args, "human_like") { return false; }
            if has_arg(args, "id") || has_arg(args, "ref") || has_arg(args, "name")
                || has_arg(args, "automation_id") || has_arg(args, "target_text")
            {
                return false;
            }
            if target_blocked(args) || !no_snapshot(args) || !native_mode_only() { return false; }
            // Sin behavior explícito el origen resuelve "auto" => proxy.
            behavior_explicit(args)
        }
        "computer_human_click" => {
            if !int_present(args, "x") || !int_present(args, "y") { return false; }
            if has_arg(args, "id") || !button_ok(args) { return false; }
            !target_blocked(args)
        }
        "computer_drag" => {
            let coords = ["fromX", "fromY", "toX", "toY"].iter().all(|k| int_present(args, k));
            coords && button_ok(args) && behavior_explicit(args) && native_mode_only()
        }
        "computer_scroll" => {
            int_or_absent(args, "x") && int_or_absent(args, "y")
                && int_or_absent(args, "dy") && int_or_absent(args, "dx")
                && bool_or_absent(args, "human_like") && behavior_explicit(args) && native_mode_only()
        }
        "computer_type" => {
            let secret_ok = match args.get("secret_b64") {
                None | Some(Value::Null) => true,
                Some(Value::String(s)) => base64::engine::general_purpose::STANDARD.decode(s).is_ok(),
                _ => false,
            };
            let plain = if let Some(Value::String(s)) = args.get("secret_b64").filter(|v| truthy_value(v)) {
                match base64::engine::general_purpose::STANDARD.decode(s) {
                    Ok(b) => String::from_utf8_lossy(&b).to_string(),
                    Err(_) => String::new(),
                }
            } else {
                args.get("text").map(cu_core::js::js_string_of).unwrap_or_default()
            };
            if plain.is_empty() || !plain_text_ok(&plain) || !secret_ok { return false; }
            if has_arg(args, "id") || has_arg(args, "ref") || has_arg(args, "name")
                || has_arg(args, "automation_id")
            {
                return false;
            }
            bool_or_absent(args, "human_jitter") && bool_or_absent(args, "human_like")
                && no_snapshot(args) && behavior_explicit(args) && native_mode_only()
        }
        "computer_fill_form" | "computer_scroll_to_text" | "computer_click_text" | "computer_upload_file" => false,
        "computer_handle_file_dialog" => args.get("file_path").map(|v| !cu_core::js::js_string_of(v).trim().is_empty()).unwrap_or(false),
        _ => false,
    }
}

fn truthy_value(v: &Value) -> bool {
    cu_core::js::js_truthy(v)
}

// ---------------------------------------------------------------------------
// RNG simple (no depende de crate externo).
// ---------------------------------------------------------------------------
// Math.round de JS: mitad hacia +infinito (-1.5 -> -1).
fn js_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

// [int] de PowerShell / Math.Round de C#: mitad al par (banker's).
fn ps_int(v: f64) -> i32 {
    v.round_ties_even() as i32
}

struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new(seed: u64) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo { return lo; }
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

fn button_flags(button: &str) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    if button == "right" { (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP) }
    else if button == "middle" { (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP) }
    else { (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP) }
}

fn send_unicode_char(ch: char, keyup: bool) {
    unsafe {
        let mut flags = KEYEVENTF_UNICODE;
        if keyup { flags = flags | KEYEVENTF_KEYUP; }
        let inp = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch as u16,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let _ = SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

fn native_human_click(x: i32, y: i32, button: &str, double: bool) -> Value {
    let mut pt = POINT::default();
    unsafe { let _ = GetCursorPos(&mut pt); }
    let start_x = pt.x;
    let start_y = pt.y;
    // PORT NOTE: origin [math]::Max(10,[math]::Min(35,[int]($dist/35))) y [int] de PowerShell (redondeo).
    let dist = ((x - start_x) as f64).hypot((y - start_y) as f64);
    let steps = ps_int(dist / 35.0).max(10).min(35);
    let mut rng = SimpleRng::new((start_x as u64).wrapping_mul(31).wrapping_add(start_y as u64));
    let dev = ps_int(dist * 0.18).min(100).max(0);
    let ctrl1x = start_x as f64 + (x - start_x) as f64 * 0.3 + rng.range(-dev, dev) as f64;
    let ctrl1y = start_y as f64 + (y - start_y) as f64 * 0.3 + rng.range(-dev, dev) as f64;
    let ctrl2x = start_x as f64 + (x - start_x) as f64 * 0.7 + rng.range(-dev, dev) as f64;
    let ctrl2y = start_y as f64 + (y - start_y) as f64 * 0.7 + rng.range(-dev, dev) as f64;

    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let u = 1.0 - t;
        let tt = t * t;
        let uu = u * u;
        let uuu = uu * u;
        let ttt = tt * t;
        let bx = ps_int(uuu * start_x as f64 + 3.0 * uu * t * ctrl1x
            + 3.0 * u * tt * ctrl2x + ttt * x as f64);
        let by = ps_int(uuu * start_y as f64 + 3.0 * uu * t * ctrl1y
            + 3.0 * u * tt * ctrl2y + ttt * y as f64);
        unsafe { let _ = SetCursorPos(bx, by); }
        thread::sleep(Duration::from_millis(rng.range(5, 14) as u64));
    }
    unsafe { let _ = SetCursorPos(x, y); }
    thread::sleep(Duration::from_millis(rng.range(30, 60) as u64));

    let (down, up) = button_flags(button);
    unsafe {
        mouse_event(down, 0, 0, 0, 0);
    }
    thread::sleep(Duration::from_millis(rng.range(65, 120) as u64));
    unsafe {
        mouse_event(up, 0, 0, 0, 0);
    }
    if double {
        thread::sleep(Duration::from_millis(rng.range(80, 130) as u64));
        unsafe { mouse_event(down, 0, 0, 0, 0); }
        thread::sleep(Duration::from_millis(rng.range(50, 95) as u64));
        unsafe { mouse_event(up, 0, 0, 0, 0); }
    }
    json!({"ok": true, "x": x, "y": y})
}

fn native_human_drag(args: &Value) {
    let from_x = int_arg(args.get("fromX"), -100000, 100000, 0) as i32;
    let from_y = int_arg(args.get("fromY"), -100000, 100000, 0) as i32;
    let to_x = int_arg(args.get("toX"), -100000, 100000, 0) as i32;
    let to_y = int_arg(args.get("toY"), -100000, 100000, 0) as i32;
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string()).to_lowercase();
    let button = if button.is_empty() { "left".to_string() } else { button };
    let (down, up) = button_flags(&button);
    let mut rng = SimpleRng::new((from_x as u64).wrapping_add(from_y as u64));
    unsafe {
        let _ = SetCursorPos(from_x, from_y);
    }
    thread::sleep(Duration::from_millis(rng.range(40, 90) as u64));
    unsafe { mouse_event(down, 0, 0, 0, 0); }
    thread::sleep(Duration::from_millis(rng.range(50, 110) as u64));
    let dist = ((to_x - from_x) as f64).hypot((to_y - from_y) as f64);
    let steps = ps_int(dist / 22.0).max(10).min(28);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let smooth = t * t * (3.0 - 2.0 * t);
        let jx = if i < steps { rng.range(-3, 3) } else { 0 };
        let jy = if i < steps { rng.range(-3, 3) } else { 0 };
        let cur_x = ps_int(from_x as f64 + (to_x - from_x) as f64 * smooth) + jx;
        let cur_y = ps_int(from_y as f64 + (to_y - from_y) as f64 * smooth) + jy;
        unsafe { let _ = SetCursorPos(cur_x, cur_y); }
        thread::sleep(Duration::from_millis(rng.range(8, 22) as u64));
    }
    unsafe { let _ = SetCursorPos(to_x, to_y); }
    thread::sleep(Duration::from_millis(rng.range(50, 110) as u64));
    unsafe { mouse_event(up, 0, 0, 0, 0); }
}

fn native_human_scroll(args: &Value) {
    let dy = int_arg(args.get("dy"), -100000, 100000, 0);
    let dx = int_arg(args.get("dx"), -100000, 100000, 0);
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    let total = dy.abs().max(dx.abs()).max(1);
    // PORT NOTE: origin Math.ceil(total/120) acotado a [1,8]; reparto con Math.round de JS (mitad hacia +inf).
    let steps = ((total + 119) / 120).max(1).min(8) as usize;
    let mut rest_y = dy;
    let mut rest_x = dx;
    for i in (1..=steps).rev() {
        let step_y = js_round(rest_y as f64 / i as f64);
        let step_x = js_round(rest_x as f64 / i as f64);
        rest_y -= step_y;
        rest_x -= step_x;
        if step_y != 0 || step_x != 0 {
            unsafe {
                let _ = SetCursorPos(x, y);
                mouse_event(MOUSEEVENTF_WHEEL, 0, 0, step_y as i32, 0);
                if step_x != 0 {
                    mouse_event(MOUSEEVENTF_HWHEEL, 0, 0, step_x as i32, 0);
                }
            }
        }
        if i > 1 {
            thread::sleep(Duration::from_millis(35 + (SimpleRng::new(i as u64).next() % 45)));
        }
    }
}

fn native_human_type(text: &str, is_secret: bool) {
    let _ = is_secret;
    let mut rng = SimpleRng::new(text.len() as u64 + 1);
    for ch in text.chars() {
        let escaped = ch.to_string();
        // Send unicode char.
        send_unicode_char(ch, false);
        thread::sleep(Duration::from_millis(10));
        send_unicode_char(ch, true);
        let mut delay = rng.range(45, 110) as u64;
        if rng.range(0, 9) == 0 {
            delay += rng.range(80, 180) as u64;
        }
        thread::sleep(Duration::from_millis(delay));
        let _ = escaped;
    }
}

fn handle_click(args: &Value, is_double: bool) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    // acepts garantiza x,y enteros presentes; sin ellos no hay coordenada.
    if args.get("x").is_none() || args.get("y").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "x,y or id or ref is required"}), true);
    }
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    let button = args.get("button").and_then(Value::as_str).unwrap_or("left").to_string();
    let humanize = args.get("human_like") == Some(&Value::Bool(true))
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        // PORT NOTE: el origen descarta el retorno de nativeHumanClick; la forma es la de click normal.
        let _ = native_human_click(x, y, &button, is_double);
        let epoch = cu_core::epoch::bump();
        let action = if is_double { "double_click" } else { "click" };
        return ToolOut::Json(json!({"ok": true, "action": action, "x": x, "y": y, "behavior": "humanized", "epoch": epoch}), false);
    }

    unsafe {
        let _ = SetCursorPos(x, y);
        let (down, up) = button_flags(&button);
        if is_double {
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
        } else {
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(20));
            mouse_event(up, 0, 0, 0, 0);
        }
    }
    let epoch = cu_core::epoch::bump();
    let action = if is_double { "double_click" } else { "click" };
    let payload = json!({"ok": true, "action": action, "x": x, "y": y, "behavior": "foreground", "epoch": epoch});
    ToolOut::Json(payload, false)
}

fn handle_human_click(args: &Value) -> ToolOut {
    if let Some(tw) = args.get("target_window") {
        if !tw.is_null() { unsafe { focus_window(tw) }; }
    }
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    if args.get("x").is_none() || args.get("y").is_none() {
        return ToolOut::Json(json!({"ok": false, "error": "x,y or id is required"}), true);
    }
    let button = args.get("button").map(cu_core::js::js_string_of).unwrap_or_else(|| "left".to_string());
    let button = if button.is_empty() { "left".to_string() } else { button };
    let double = args.get("double_click").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false);
    let res = native_human_click(x, y, &button, double);
    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "human_click": res, "epoch": epoch}), false)
}

// PORT NOTE: el origen no enfoca la ventana en drag/scroll/type (solo click/human_click/copy/paste/bulk).
// humanize = plan.mode === "humanized" (sin human_like), con behavior explícito.
fn handle_drag(args: &Value) -> ToolOut {
    let from_x = int_arg(args.get("fromX"), -100000, 100000, 0) as i32;
    let from_y = int_arg(args.get("fromY"), -100000, 100000, 0) as i32;
    let to_x = int_arg(args.get("toX"), -100000, 100000, 0) as i32;
    let to_y = int_arg(args.get("toY"), -100000, 100000, 0) as i32;
    let button = args.get("button").and_then(Value::as_str).unwrap_or("left").to_string();
    let humanize = args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        native_human_drag(args);
    } else {
        // PORT NOTE: camino primario del origen = DesktopBridge.exe `mouse drag` (10 pasos,
        // esperas 40/15/30, truncado de C# (int)). Replicado tal cual.
        let (down, up) = button_flags(&button);
        unsafe {
            let _ = SetCursorPos(from_x, from_y);
            mouse_event(down, 0, 0, 0, 0);
            thread::sleep(Duration::from_millis(40));
            let steps = 10i32;
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let smooth = t * t * (3.0 - 2.0 * t);
                let cur_x = (from_x as f64 + (to_x - from_x) as f64 * smooth) as i32;
                let cur_y = (from_y as f64 + (to_y - from_y) as f64 * smooth) as i32;
                let _ = SetCursorPos(cur_x, cur_y);
                thread::sleep(Duration::from_millis(15));
            }
            let _ = SetCursorPos(to_x, to_y);
            thread::sleep(Duration::from_millis(30));
            mouse_event(up, 0, 0, 0, 0);
        }
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if humanize { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "drag", "behavior": behavior, "epoch": epoch}), false)
}

fn handle_scroll(args: &Value) -> ToolOut {
    let x = int_arg(args.get("x"), -100000, 100000, 0) as i32;
    let y = int_arg(args.get("y"), -100000, 100000, 0) as i32;
    let dy = int_arg(args.get("dy"), -100000, 100000, 0);
    let dx = int_arg(args.get("dx"), -100000, 100000, 0);
    let humanize = args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if humanize {
        native_human_scroll(args);
    } else {
        // PORT NOTE: DesktopBridge `mouse scroll` hace SetCursorPos(x,y) siempre (x,y ausentes => 0).
        unsafe {
            let _ = SetCursorPos(x, y);
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, dy as i32, 0);
            if dx != 0 {
                mouse_event(MOUSEEVENTF_HWHEEL, 0, 0, dx as i32, 0);
            }
        }
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if humanize { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "scroll", "behavior": behavior, "epoch": epoch}), false)
}

// PORT NOTE: origin computer_type no enfoca la ventana. jitter = plan.mode==="humanized" || human_jitter===true.
fn handle_type(args: &Value) -> ToolOut {
    let mut plain_text = args.get("text").map(cu_core::js::js_string_of).unwrap_or_default();
    let is_secret = truthy(args, "is_secret") || truthy(args, "secret_b64");
    if truthy(args, "secret_b64") {
        if let Some(b64) = args.get("secret_b64").and_then(Value::as_str) {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                plain_text = String::from_utf8_lossy(&bytes).to_string();
            }
        }
    }
    if truthy(args, "clear_first") {
        native_key_action("a", "tap", &["ctrl".to_string()]);
        native_key_action("Backspace", "tap", &[]);
    }
    let jitter = args.get("human_jitter") == Some(&Value::Bool(true))
        || args.get("behavior").and_then(Value::as_str) == Some("humanized");

    if jitter {
        native_human_type(&plain_text, is_secret);
    } else {
        for ch in plain_text.chars() {
            send_unicode_char(ch, false);
            thread::sleep(Duration::from_millis(2));
            send_unicode_char(ch, true);
        }
    }
    let press_enter = args.get("press_enter").map(|v| cu_core::js::js_truthy(v)).unwrap_or(false);
    if press_enter {
        native_key_action("Enter", "tap", &[]);
    }
    let epoch = cu_core::epoch::bump();
    let behavior = if jitter { "humanized" } else { "foreground" };
    ToolOut::Json(json!({"ok": true, "action": "type", "secret": is_secret, "behavior": behavior, "human_jitter": jitter, "epoch": epoch}), false)
}

fn handle_file_dialog(args: &Value) -> ToolOut {
    let file_path = args.get("file_path").map(cu_core::js::js_string_of).unwrap_or_default().trim().to_string();
    if file_path.is_empty() {
        return ToolOut::Json(json!({"ok": false, "error": "file_path parameter is required"}), true);
    }
    let action = args.get("action").map(cu_core::js::js_string_of).unwrap_or_else(|| "save".to_string());

    // Buscar diálogo #32770.
    let mut found = HWND(std::ptr::null_mut());
    unsafe {
        struct Ctx { found: HWND }
        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            unsafe {
                let ctx = &mut *(lparam.0 as *mut Ctx);
                let mut buf = [0u16; 256];
                let n = GetClassNameW(hwnd, &mut buf);
                if n > 0 {
                    let class = String::from_utf16_lossy(&buf[..n as usize]);
                    if class == "#32770" {
                        let mut tbuf = [0u16; 256];
                        let tn = GetWindowTextW(hwnd, &mut tbuf);
                        let title = String::from_utf16_lossy(&tbuf[..tn.max(0) as usize]).to_lowercase();
                        if title.contains("guardar") || title.contains("save") || title.contains("abrir")
                            || title.contains("open") || title.contains("subir") || title.contains("upload")
                            || title.contains("select") || title.contains("elegir")
                        {
                            ctx.found = hwnd;
                            return windows::core::BOOL(0);
                        }
                    }
                }
                windows::core::BOOL(1)
            }
        }
        let mut ctx = Ctx { found: HWND(std::ptr::null_mut()) };
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        if !ctx.found.0.is_null() {
            found = ctx.found;
        } else {
            let fg = GetForegroundWindow();
            let mut cbuf = [0u16; 256];
            let cn = GetClassNameW(fg, &mut cbuf);
            if cn > 0 {
                let class = String::from_utf16_lossy(&cbuf[..cn as usize]);
                if class == "#32770" {
                    found = fg;
                }
            }
        }
    }

    if found.0.is_null() {
        return ToolOut::Json(json!({"ok": false, "error": "file_dialog_not_found"}), true);
    }

    unsafe {
        let _ = SetForegroundWindow(found);
    }
    thread::sleep(Duration::from_millis(80));

    let _ = cu_native::system::clipboard_set(&file_path);
    thread::sleep(Duration::from_millis(30));
    native_key_action("v", "tap", &["ctrl".to_string()]);
    thread::sleep(Duration::from_millis(120));
    native_key_action("Enter", "tap", &[]);

    let epoch = cu_core::epoch::bump();
    ToolOut::Json(json!({"ok": true, "dialog_hwnd": found.0 as isize as i64, "path": file_path, "action": action, "epoch": epoch}), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_hotkey_basic() {
        let (key, mods) = parse_hotkey("Ctrl+Shift+C");
        assert_eq!(key, "c");
        assert_eq!(mods, vec!["ctrl".to_string(), "shift".to_string()]);
    }

    #[test]
    fn parse_hotkey_win() {
        let (key, mods) = parse_hotkey("win+r");
        assert_eq!(key, "r");
        assert_eq!(mods, vec!["meta".to_string()]);
    }

    #[test]
    fn int_clamp_works() {
        assert_eq!(int_arg(Some(&json!(5.9)), 0, 10, 0), 5);
        assert_eq!(int_arg(Some(&json!("abc")), 0, 10, 3), 3);
        assert_eq!(int_arg(Some(&json!(100)), 0, 10, 0), 10);
        assert_eq!(int_arg(Some(&json!(-5)), 0, 10, 0), 0);
    }

    #[test]
    fn get_vk_letters() {
        assert_eq!(get_vk("a"), 0x41);
        assert_eq!(get_vk("Z"), 0x5A);
        assert_eq!(get_vk("Enter"), 0x0D);
        assert_eq!(get_vk("F5"), 0x74);
    }

    #[test]
    fn behavior_auto_is_not_accepted() {
        // P0 F7: behavior "auto" (o ausente) lo resuelve el origen por ventana/UIA/env => proxy.
        let auto = json!({"x": 10, "y": 20, "behavior": "auto"});
        let absent = json!({"x": 10, "y": 20});
        for tool in ["computer_click", "computer_double_click", "computer_scroll", "computer_type"] {
            assert!(!accepts(tool, &auto), "{tool} acepta auto");
            assert!(!accepts(tool, &absent), "{tool} acepta behavior ausente");
        }
        let drag_auto = json!({"fromX": 1, "fromY": 2, "toX": 3, "toY": 4, "behavior": "auto"});
        assert!(!accepts("computer_drag", &drag_auto));
        assert!(!accepts("computer_drag", &json!({"fromX": 1, "fromY": 2, "toX": 3, "toY": 4})));
    }

    #[test]
    fn explicit_behavior_accepted() {
        assert!(accepts("computer_click", &json!({"x": 10, "y": 20, "behavior": "foreground"})));
        assert!(accepts("computer_click", &json!({"x": 10, "y": 20, "behavior": "humanized"})));
        assert!(!accepts("computer_click", &json!({"x": 10, "y": 20, "behavior": "headless"})));
        assert!(!accepts("computer_click", &json!({"x": 10, "y": 20, "behavior": "Foreground"})));
        assert!(accepts("computer_type", &json!({"text": "hi", "behavior": "foreground"})));
        assert!(!accepts("computer_type", &json!({"text": "hi", "behavior": "foreground", "id": 3})));
        assert!(accepts("computer_drag", &json!({"fromX": 1, "fromY": 2, "toX": 3, "toY": 4, "behavior": "foreground"})));
        assert!(accepts("computer_scroll", &json!({"dy": -120, "behavior": "humanized"})));
    }

    #[test]
    fn click_rejects_non_integer_and_flags() {
        let base = json!({"behavior": "foreground"});
        let mut f = base.clone();
        f["x"] = json!(10.5);
        f["y"] = json!(20);
        assert!(!accepts("computer_click", &f));
        let mut s = base.clone();
        s["x"] = json!("10");
        s["y"] = json!(20);
        assert!(!accepts("computer_click", &s));
        let mut h = base.clone();
        h["x"] = json!(10);
        h["y"] = json!(20);
        h["human_like"] = json!("true");
        assert!(!accepts("computer_click", &h));
        let mut b = base.clone();
        b["x"] = json!(10);
        b["y"] = json!(20);
        b["button"] = json!("RIGHT");
        assert!(!accepts("computer_click", &b));
        let mut sn = base;
        sn["x"] = json!(10);
        sn["y"] = json!(20);
        sn["snapshot_after"] = json!(true);
        assert!(!accepts("computer_click", &sn));
    }

    #[test]
    fn blocked_password_manager_target_is_not_accepted() {
        let v = json!({"x": 1, "y": 2, "behavior": "foreground", "target_window": "Bitwarden - Vault"});
        assert!(!accepts("computer_click", &v));
        assert!(!accepts("computer_human_click", &json!({"x": 1, "y": 2, "target_window": "1Password"})));
        assert!(!accepts("computer_copy", &json!({"target_window": "KeePass"})));
        assert!(accepts("computer_copy", &json!({"target_window": "Notepad"})));
    }

    #[test]
    fn type_rejects_control_chars_and_unsupported_flags() {
        assert!(!accepts("computer_type", &json!({"text": "a\nb", "behavior": "foreground"})));
        assert!(!accepts("computer_type", &json!({"text": "hi", "behavior": "foreground", "snapshot_after": true})));
        assert!(!accepts("computer_type", &json!({"text": "hi", "behavior": "foreground", "human_jitter": "yes"})));
        assert!(!accepts("computer_type", &json!({"secret_b64": "%%%", "behavior": "foreground"})));
        assert!(accepts("computer_type", &json!({"secret_b64": "aGk=", "behavior": "foreground"})));
    }

    #[test]
    fn hotkey_requires_string_combo() {
        assert!(accepts("computer_hotkey", &json!({"combo": "ctrl+c"})));
        assert!(!accepts("computer_hotkey", &json!({"combo": 0})));
        assert!(!accepts("computer_hotkey", &json!({"combo": "  "})));
    }

    #[test]
    fn bulk_input_cells_and_target_text() {
        assert!(accepts("computer_bulk_input", &json!({"rows": [["a", 1, null]]})));
        assert!(!accepts("computer_bulk_input", &json!({"rows": [[true]]})));
        assert!(!accepts("computer_bulk_input", &json!({"rows": [["a"]], "target_text": "x"})));
        assert!(!accepts("computer_bulk_input", &json!({"rows": [["a"]], "target_x": 0, "target_y": 0, "target_text": "x"})));
        assert!(accepts("computer_bulk_input", &json!({"rows": [["a"]], "target_x": 5, "target_y": 6, "target_text": "x"})));
    }

    #[test]
    fn drag_scroll_accepts_negative_steps_as_default() {
        assert!(accepts("computer_drag_scroll", &json!({"from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4, "steps": -5})));
        assert!(!accepts("computer_drag_scroll", &json!({"from_x": 1.5, "from_y": 2, "to_x": 3, "to_y": 4})));
    }

    #[test]
    fn unsupported_tools_are_not_accepted() {
        assert!(!accepts("computer_fill_form", &json!({})));
        assert!(!accepts("computer_scroll_to_text", &json!({"text":"x"})));
        assert!(!accepts("computer_click_text", &json!({"text":"x"})));
        assert!(!accepts("computer_upload_file", &json!({"file_path":"x"})));
    }
}
