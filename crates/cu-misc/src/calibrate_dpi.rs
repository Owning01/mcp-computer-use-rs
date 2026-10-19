// cu-misc::calibrate_dpi — port de computer_calibrate_dpi (calibrate_dpi.js).
use serde_json::{json, Value};
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::core::BOOL;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenDesktopW, SetThreadDesktop};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetAwarenessFromDpiAwarenessContext, GetDpiForMonitor,
    GetDpiForSystem, GetDpiForWindow, GetWindowDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    DPI_AWARENESS_CONTEXT_UNAWARE, DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED, MONITOR_DPI_TYPE,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MonitorFromWindow, ScreenToClient};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetForegroundWindow, GetWindowRect, GetWindowTextW, IsWindowVisible,
};

pub fn calibrate_dpi(args: &Value) -> Value {
    // parse inputs like handle()
    let mut in_x: Option<i64> = args.get("x").and_then(|v| v.as_f64()).map(|n| n as i64);
    let mut in_y: Option<i64> = args.get("y").and_then(|v| v.as_f64()).map(|n| n as i64);
    if in_x.is_none() {
        if let Some(p) = args.get("point") {
            in_x = p.get("x").and_then(|v| v.as_f64()).map(|n| n as i64);
        }
    }
    if in_y.is_none() {
        if let Some(p) = args.get("point") {
            in_y = p.get("y").and_then(|v| v.as_f64()).map(|n| n as i64);
        }
    }
    let mut from_type = args
        .get("from_coord_type")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_lowercase();
    let mut to_type = args
        .get("to_coord_type")
        .and_then(|v| v.as_str())
        .unwrap_or("physical")
        .to_lowercase();
    if let Some(c) = args.get("convert").and_then(|v| v.as_str()) {
        match c {
            "logical_to_physical" => {
                from_type = "logical".into();
                to_type = "physical".into();
            }
            "physical_to_logical" => {
                from_type = "physical".into();
                to_type = "logical".into();
            }
            "client_to_screen" => {
                from_type = "client".into();
                to_type = "screen".into();
            }
            "screen_to_client" => {
                from_type = "screen".into();
                to_type = "client".into();
            }
            _ => {}
        }
    }
    let target_title = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let hwnd_val = args.get("hwnd").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;

    // Run on a thread with STA apartment (like origin) for desktop attach.
    let handle = std::thread::spawn(move || {
        calibrate_inner(hwnd_val, &target_title, in_x, in_y, &from_type, &to_type)
    });
    match handle.join() {
        Ok(v) => v,
        Err(_) => json!({"ok": false, "error": "thread panicked"}),
    }
}

fn calibrate_inner(
    hwnd_val: i64,
    target_title: &str,
    in_x: Option<i64>,
    in_y: Option<i64>,
    from_type: &str,
    to_type: &str,
) -> Value {
    unsafe {
        // Open default desktop
        let desktop_name: Vec<u16> = "default\0".encode_utf16().collect();
        let h_desk = OpenDesktopW(
            windows::core::PCWSTR(desktop_name.as_ptr()),
            windows::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS(0),
            false,
            0x01FF,
        )
        .ok();

        // We'll run logic directly on this thread; set desktop if opened
        if let Some(d) = h_desk {
            let _ = SetThreadDesktop(d);
        }

        let result = (|| -> Value {
            // find target hwnd
            let mut hwnd = if hwnd_val != 0 {
                HWND(hwnd_val as *mut c_void)
            } else {
                HWND(std::ptr::null_mut())
            };
            if hwnd.0.is_null() && !target_title.is_empty() {
                hwnd = find_target_window(target_title);
            }
            if hwnd.0.is_null() {
                hwnd = GetForegroundWindow();
            }
            if hwnd.0.is_null() {
                return json!({"ok": false, "error": "no target window"});
            }

            // window rect
            let mut win_rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut win_rect);
            let mut client_rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut client_rect);

            // title
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf);
            let title = if len > 0 {
                String::from_utf16_lossy(&buf[..len as usize])
            } else {
                String::new()
            };

            // awareness
            let mut awareness = "Unknown".to_string();
            let mut awareness_code = 0i32;
            let ctx = GetWindowDpiAwarenessContext(hwnd);
            if AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool() {
                awareness = "Per-Monitor v2".into();
                awareness_code = 2;
            } else if AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE).as_bool() {
                awareness = "Per-Monitor v1".into();
                awareness_code = 2;
            } else if AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE).as_bool() {
                awareness = "System DPI".into();
                awareness_code = 1;
            } else if AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_UNAWARE).as_bool() {
                awareness = "DPI Unaware".into();
                awareness_code = 0;
            } else if AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED).as_bool() {
                awareness = "DPI Unaware (GDI Scaled)".into();
                awareness_code = 0;
            } else {
                let raw = GetAwarenessFromDpiAwarenessContext(ctx).0;
                awareness_code = raw;
                if raw == 0 {
                    awareness = "DPI Unaware".into();
                } else if raw == 1 {
                    awareness = "System DPI".into();
                } else if raw == 2 {
                    awareness = "Per-Monitor".into();
                }
            }

            // window dpi
            let mut w_dpi = GetDpiForWindow(hwnd);
            if w_dpi == 0 {
                w_dpi = 96;
            }
            let window_scale = (w_dpi as f64 / 96.0 * 100.0).round() / 100.0;

            // system dpi
            let mut s_dpi = GetDpiForSystem();
            if s_dpi == 0 {
                s_dpi = 96;
            }
            let system_scale = (s_dpi as f64 / 96.0 * 100.0).round() / 100.0;

            // monitor
            let hmon: HMONITOR = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFOEXW {
                monitorInfo: windows::Win32::Graphics::Gdi::MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let _ = GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut _);
            let mon_name_len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            let monitor_device = String::from_utf16_lossy(&mi.szDevice[..mon_name_len]);

            let mut m_dpi_x: u32 = 96;
            let mut m_dpi_y: u32 = 96;
            let _ = GetDpiForMonitor(hmon, MONITOR_DPI_TYPE(0), &mut m_dpi_x, &mut m_dpi_y);
            if m_dpi_x == 0 {
                m_dpi_x = 96;
            }
            let monitor_scale = (m_dpi_x as f64 / 96.0 * 100.0).round() / 100.0;

            // coords
            let has_coords = in_x.is_some() && in_y.is_some();
            let mut cal_x = in_x.unwrap_or(0) as i32;
            let mut cal_y = in_y.unwrap_or(0) as i32;
            if has_coords {
                let ix = in_x.unwrap() as i32;
                let iy = in_y.unwrap() as i32;
                cal_x = ix;
                cal_y = iy;
                if from_type == "client" {
                    let mut pt = windows::Win32::Foundation::POINT { x: ix, y: iy };
                    let _ = ClientToScreen(hwnd, &mut pt);
                    cal_x = pt.x;
                    cal_y = pt.y;
                } else if from_type == "screen" && to_type == "client" {
                    let mut pt = windows::Win32::Foundation::POINT { x: ix, y: iy };
                    let _ = ScreenToClient(hwnd, &mut pt);
                    cal_x = pt.x;
                    cal_y = pt.y;
                } else if from_type == "logical" && to_type == "physical" {
                    let s = if window_scale > 0.0 { window_scale } else { 1.0 };
                    cal_x = (ix as f64 * s).round() as i32;
                    cal_y = (iy as f64 * s).round() as i32;
                } else if from_type == "physical" && to_type == "logical" {
                    let s = if window_scale > 0.0 { window_scale } else { 1.0 };
                    cal_x = (ix as f64 / s).round() as i32;
                    cal_y = (iy as f64 / s).round() as i32;
                } else if from_type == "auto" && awareness == "DPI Unaware" {
                    let s = monitor_scale;
                    cal_x = (ix as f64 * s).round() as i32;
                    cal_y = (iy as f64 * s).round() as i32;
                }
            }

            json!({
                "ok": true,
                "hwnd": hwnd.0 as isize as i64,
                "title": title,
                "awareness": awareness,
                "awareness_code": awareness_code,
                "window_dpi": w_dpi,
                "window_scale": window_scale,
                "system_dpi": s_dpi,
                "system_scale": system_scale,
                "monitor_device": monitor_device,
                "monitor_dpi": m_dpi_x,
                "monitor_scale": monitor_scale,
                "mon_x": mi.monitorInfo.rcMonitor.left,
                "mon_y": mi.monitorInfo.rcMonitor.top,
                "mon_w": mi.monitorInfo.rcMonitor.right - mi.monitorInfo.rcMonitor.left,
                "mon_h": mi.monitorInfo.rcMonitor.bottom - mi.monitorInfo.rcMonitor.top,
                "work_x": mi.monitorInfo.rcWork.left,
                "work_y": mi.monitorInfo.rcWork.top,
                "work_w": mi.monitorInfo.rcWork.right - mi.monitorInfo.rcWork.left,
                "work_h": mi.monitorInfo.rcWork.bottom - mi.monitorInfo.rcWork.top,
                "win_x": win_rect.left,
                "win_y": win_rect.top,
                "win_w": win_rect.right - win_rect.left,
                "win_h": win_rect.bottom - win_rect.top,
                "client_w": client_rect.right - client_rect.left,
                "client_h": client_rect.bottom - client_rect.top,
                "converted_point": { "x": cal_x, "y": cal_y },
                "calibrated_x": cal_x,
                "calibrated_y": cal_y,
                "from_type": from_type,
                "to_type": to_type
            })
        })();

        if let Some(d) = h_desk {
            let _ = CloseDesktop(d);
        }
        result
    }
}

fn find_target_window(title: &str) -> HWND {
    struct Ctx {
        target: String,
        found: Option<HWND>,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                let t = String::from_utf16_lossy(&buf[..len as usize]);
                if !ctx.target.is_empty() && t.to_lowercase().contains(&ctx.target.to_lowercase()) {
                    ctx.found = Some(hwnd);
                    return BOOL(0);
                }
            }
            BOOL(1)
        }
    }
    let mut ctx = Ctx {
        target: title.to_string(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.found.unwrap_or(HWND(std::ptr::null_mut()))
}