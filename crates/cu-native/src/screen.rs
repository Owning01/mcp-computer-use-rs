// cu-native::screen â€” port de computer_screen_info (F6, repo-migrate).
// Contrato: nativeScreenInfo() de index.js (PS + System.Windows.Forms.Screen).
// DPI: el origen corre en un proceso DPI-unaware -> dpi 96 / scale 1 / cursor virtualizado.
// Este thread fija UNAWARE para replicarlo (igual que screenshot).
use serde_json::{Value, json};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BITSPIXEL, EnumDisplayMonitors, GetDC, GetDeviceCaps, GetMonitorInfoW, HDC, LOGPIXELSX,
    LOGPIXELSY, MONITORINFO, MONITORINFOEXW, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

pub fn screen_info() -> Value {
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    struct Ctx {
        monitors: Vec<(MONITORINFOEXW, bool)>,
        min_l: i32,
        min_t: i32,
        max_r: i32,
        max_b: i32,
    }
    unsafe extern "system" fn cb(
        hmon: windows::Win32::Graphics::Gdi::HMONITOR,
        _hdc: HDC,
        _rc: *mut windows::Win32::Foundation::RECT,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> windows::core::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let mut mi = MONITORINFOEXW {
                monitorInfo: MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
                let primary = mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
                let r = mi.monitorInfo.rcMonitor;
                ctx.min_l = ctx.min_l.min(r.left);
                ctx.min_t = ctx.min_t.min(r.top);
                ctx.max_r = ctx.max_r.max(r.right);
                ctx.max_b = ctx.max_b.max(r.bottom);
                ctx.monitors.push((mi, primary));
            }
            windows::core::BOOL(1)
        }
    }

    let mut ctx = Ctx {
        monitors: Vec::new(),
        min_l: 0,
        min_t: 0,
        max_r: 0,
        max_b: 0,
    };
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(cb),
            windows::Win32::Foundation::LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }

    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hdc = GetDC(None);
        let dpi_x = GetDeviceCaps(Some(hdc), LOGPIXELSX);
        let dpi_y = GetDeviceCaps(Some(hdc), LOGPIXELSY);
        let bits = GetDeviceCaps(Some(hdc), BITSPIXEL);
        ReleaseDC(None, hdc);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        let scale = f64::round((dpi_x as f64 / 96.0) * 100.0) / 100.0;
        let mut monitors = Vec::new();
        let mut primary_res = (0i32, 0i32);
        for (idx, (mi, primary)) in ctx.monitors.iter().enumerate() {
            let r = mi.monitorInfo.rcMonitor;
            let wa = mi.monitorInfo.rcWork;
            let name_len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            let device_name = String::from_utf16_lossy(&mi.szDevice[..name_len]);
            if *primary {
                primary_res = (r.right - r.left, r.bottom - r.top);
            }
            monitors.push(json!({
                "bounds": { "y": r.top, "width": r.right - r.left, "height": r.bottom - r.top, "x": r.left },
                "device_name": device_name,
                "working_area": { "y": wa.top, "width": wa.right - wa.left, "height": wa.bottom - wa.top, "x": wa.left },
                "primary": primary,
                "bits_per_pixel": bits,
                "index": idx
            }));
        }
        let count = monitors.len();
        // PORT NOTE: orden de claves calcado del ConvertTo-Json de PS 5.1 (observado estable en esta maquina).
        json!({
            "monitors": monitors,
            "dpi_x": dpi_x,
            "cursor": { "y": pt.y, "x": pt.x },
            "scale_percent": (scale * 100.0) as i64,
            "dpi_y": dpi_y,
            "monitor_count": count,
            "virtual_screen": { "left": ctx.min_l, "width": ctx.max_r - ctx.min_l, "top": ctx.min_t, "height": ctx.max_b - ctx.min_t },
            "scale_factor": scale,
            "primary_resolution": { "height": primary_res.1, "width": primary_res.0 }
        })
    }
}
