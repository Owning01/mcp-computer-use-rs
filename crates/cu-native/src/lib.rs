// cu-native — primitivas Win32 nativas (F4/F5/F6: list_windows, capture, screen).
// Reemplaza las implementaciones PowerShell/C# del origen. Reglas: migration/RULEBOOK.md.
pub mod capture;
pub mod screen;
pub mod system;
use serde::Serialize;

/// SetThreadExecutionState (computer_awake). Devuelve false si Win32 falla.
pub fn set_keep_awake(enabled: bool, display: bool) -> bool {
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE,
        SetThreadExecutionState,
    };
    let mut flags = ES_CONTINUOUS.0;
    if enabled {
        flags |= ES_SYSTEM_REQUIRED.0;
        if display {
            flags |= ES_DISPLAY_REQUIRED.0;
        }
    }
    unsafe { SetThreadExecutionState(EXECUTION_STATE(flags)).0 != 0 }
}
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};

#[derive(Debug, Serialize)]
pub struct WindowInfo {
    pub hwnd: i64,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub pid: u32,
    pub is_minimized: bool,
}

struct CollectCtx {
    out: Vec<WindowInfo>,
    filter: Option<String>,
}

/// Misma semantica que ListWindowsJson del origen:
/// visible + titulo no vacio; si no esta minimizada, descarta dims < 50;
/// filtro case-insensitive sobre el titulo; titulo limpiado (controles fuera, tab -> espacio).
pub fn list_windows(filter: Option<&str>) -> Vec<WindowInfo> {
    // DPI: el origen (via su api nativa) reporta pixeles FISICOS; este thread se marca
    // PER_MONITOR_AWARE_V2 para replicarlo. El thread de captura fija UNAWARE (como el PS
    // del origen). Contexto explicito por llamada: los threads del pool de spawn_blocking se reusan.
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut ctx = CollectCtx {
        out: Vec::new(),
        filter: filter.map(|s| s.to_lowercase()).filter(|s| !s.is_empty()),
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut CollectCtx as isize));
    }
    ctx.out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam proviene de list_windows (puntero valido a CollectCtx durante la enumeracion);
    // las funciones Win32 se llaman con un HWND provisto por EnumWindows en el mismo hilo.
    unsafe {
        let ctx = &mut *(lparam.0 as *mut CollectCtx);

        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }

        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.trim().is_empty() {
            return BOOL(1);
        }

        let is_iconic = IsIconic(hwnd).as_bool();
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if !is_iconic && (w < 50 || h < 50) {
            return BOOL(1);
        }

        if let Some(f) = &ctx.filter {
            if !title.to_lowercase().contains(f.as_str()) {
                return BOOL(1);
            }
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let clean: String = title
            .chars()
            .filter_map(|c| {
                let code = c as u32;
                if c == '\t' {
                    Some(' ')
                } else if code >= 32 && code != 127 {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();

        ctx.out.push(WindowInfo {
            hwnd: hwnd.0 as isize as i64,
            title: clean,
            x: rect.left,
            y: rect.top,
            w,
            h,
            pid,
            is_minimized: is_iconic,
        });
        BOOL(1)
    }
}
