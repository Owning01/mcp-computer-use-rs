// PORT NOTE: computer_monitor_action portado con GDI (screen capture via cu-native::capture).
// screenshot_monitor usa captura GDI de la region del monitor (calidad JPEG 75).
use crate::focus::focus_window;
use cu_native::capture::{screenshot, ShotArgs, ShotOut};
use serde_json::{json, Value};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, EnumDisplayMonitors, GetDeviceCaps, GetMonitorInfoW, BITSPIXEL,
    MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

#[derive(Clone)]
pub struct Monitor {
    pub index: usize,
    pub device_name: String,
    pub primary: bool,
    pub bounds_x: i32,
    pub bounds_y: i32,
    pub bounds_w: i32,
    pub bounds_h: i32,
    pub work_x: i32,
    pub work_y: i32,
    pub work_w: i32,
    pub work_h: i32,
    pub bits_per_pixel: i32,
}

pub fn list_monitors() -> Vec<Monitor> {
    struct Ctx {
        mons: Vec<Monitor>,
        idx: usize,
    }
    unsafe extern "system" fn cb(hmon: HMONITOR, _hdc: windows::Win32::Graphics::Gdi::HDC, _lprc: *mut RECT, lparam: LPARAM) -> windows::core::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            ctx.idx += 1;
            let mut miex = MONITORINFOEXW::default();
            miex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            let ok = GetMonitorInfoW(hmon, &mut miex.monitorInfo as *mut _ as *mut windows::Win32::Graphics::Gdi::MONITORINFO);
            let (bx, by, bw, bh, wx, wy, ww, wh, primary) = if ok.as_bool() {
                let m = &miex.monitorInfo;
                let b = m.rcMonitor;
                let w = m.rcWork;
                (
                    b.left,
                    b.top,
                    b.right - b.left,
                    b.bottom - b.top,
                    w.left,
                    w.top,
                    w.right - w.left,
                    w.bottom - w.top,
                    (m.dwFlags & MONITORINFOF_PRIMARY) != 0,
                )
            } else {
                (0, 0, 0, 0, 0, 0, 0, 0, false)
            };
            // device name
            let mut name = String::new();
            for &c in &miex.szDevice {
                if c == 0 {
                    break;
                }
                name.push(char::from_u32(c as u32).unwrap_or('?'));
            }
            // bits per pixel
            let mut bpp = 32;
            let driver: Vec<u16> = "DISPLAY\0".encode_utf16().collect();
            let device: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let hdc = CreateDCW(windows::core::PCWSTR(driver.as_ptr()), windows::core::PCWSTR(device.as_ptr()), None, None);
            if !hdc.0.is_null() {
                bpp = GetDeviceCaps(Some(hdc), BITSPIXEL);
                let _ = DeleteDC(hdc);
            }
            ctx.mons.push(Monitor {
                index: ctx.idx,
                device_name: name,
                primary,
                bounds_x: bx,
                bounds_y: by,
                bounds_w: bw,
                bounds_h: bh,
                work_x: wx,
                work_y: wy,
                work_w: ww,
                work_h: wh,
                bits_per_pixel: bpp,
            });
            windows::core::BOOL(1)
        }
    }
    let mut ctx = Ctx { mons: Vec::new(), idx: 0 };
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.mons
}

pub fn monitor_action(args: &Value) -> Value {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list").trim().to_string();
    let monitor_index = args.get("monitor_index").and_then(|v| v.as_i64()).unwrap_or(1).clamp(1, 64) as usize;
    let target_window = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !target_window.is_empty() {
        let _ = focus_window(&json!({"title": target_window}));
    }

    let mons = list_monitors();

    if action == "list" {
        let list: Vec<Value> = mons
            .iter()
            .map(|m| {
                json!({
                    "index": m.index,
                    "device_name": m.device_name,
                    "primary": m.primary,
                    "bounds": {"x": m.bounds_x, "y": m.bounds_y, "width": m.bounds_w, "height": m.bounds_h},
                    "working_area": {"x": m.work_x, "y": m.work_y, "width": m.work_w, "height": m.work_h},
                    "bits_per_pixel": m.bits_per_pixel
                })
            })
            .collect();
        let epoch = cu_core::epoch::bump();
        return json!({"ok": true, "count": mons.len(), "monitors": list, "epoch": epoch});
    }

    if monitor_index < 1 || monitor_index > mons.len() {
        return json!({"ok": false, "error": "invalid_monitor_index", "max_monitors": mons.len()});
    }
    let target = &mons[monitor_index - 1];

    if action == "screenshot_monitor" {
        // handled by monitor_screenshot() in handle_call
        return json!({"ok": false, "error": "use_monitor_screenshot"});
    }

    if action == "move_to_monitor" {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, SW_RESTORE, SetWindowPos, ShowWindowAsync};
            use windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS;
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                return json!({"ok": false, "error": "no_active_window"});
            }
            if IsIconic(fg).as_bool() {
                let _ = ShowWindowAsync(fg, SW_RESTORE);
            }
            let new_x = target.work_x + 50;
            let new_y = target.work_y + 50;
            let _ = SetWindowPos(fg, None, new_x, new_y, 0, 0, SET_WINDOW_POS_FLAGS(0x0001));
            let epoch = cu_core::epoch::bump();
            return json!({"ok": true, "action": "move_to_monitor", "monitor_index": monitor_index, "hwnd": fg.0 as isize as i64, "x": new_x, "y": new_y, "epoch": epoch});
        }
    }

    json!({"ok": false, "error": "unknown_action", "action": action})
}

pub fn monitor_screenshot(args: &Value) -> cu_core::out::ToolOut {
    use cu_core::out::ToolOut;
    let monitor_index = args.get("monitor_index").and_then(|v| v.as_i64()).unwrap_or(1).clamp(1, 64) as usize;
    let target_window = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !target_window.is_empty() {
        let _ = focus_window(&json!({"title": target_window}));
    }
    let mons = list_monitors();
    if monitor_index < 1 || monitor_index > mons.len() {
        return ToolOut::Json(json!({"ok": false, "error": "invalid_monitor_index", "max_monitors": mons.len()}), true);
    }
    let target = &mons[monitor_index - 1];
    let shot_args = ShotArgs {
        width: 0,
        format: "jpeg".to_string(),
        quality: 75,
        hwnd: 0,
        x: target.bounds_x as i64,
        y: target.bounds_y as i64,
        w: target.bounds_w as i64,
        h: target.bounds_h as i64,
        screen: 0,
        capture: "gdi".to_string(),
        interpolation: "lanczos".to_string(),
    };
    match screenshot(&shot_args) {
        ShotOut::Ok(ok) => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&ok.bytes);
            let bounds = json!({"x": target.bounds_x, "y": target.bounds_y, "width": target.bounds_w, "height": target.bounds_h});
            let epoch = cu_core::epoch::current();
            let text = serde_json::to_string(&json!({"ok": true, "action": "screenshot_monitor", "monitor_index": monitor_index, "bounds": bounds, "epoch": epoch})).unwrap_or_default();
            ToolOut::Image { mime: "image/jpeg".to_string(), data_b64: b64, text }
        }
        ShotOut::Err { error, message, .. } => {
            ToolOut::Json(json!({"ok": false, "error": error, "message": message}), true)
        }
    }
}
