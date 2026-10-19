// cu-misc::check_health — port de nativeCheckHealth (index.js).
use serde_json::{json, Value};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsHungAppWindow};

pub fn check_health(args: &Value) -> Value {
    let target_hwnd = args.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
    let auto_alert = args.get("auto_alert").and_then(|v| v.as_bool()).unwrap_or(true);

    unsafe {
        // UAC / secure desktop detection
        use windows::Win32::System::StationsAndDesktops::OpenInputDesktop;
        // OpenInputDesktop signature: (dwFlags, fInherit, dwDesiredAccess) -> Result<HDESK>
        let mut is_uac = false;
        use windows::Win32::System::StationsAndDesktops::CloseDesktop;
        match OpenInputDesktop(windows::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS(0), false, windows::Win32::System::StationsAndDesktops::DESKTOP_ACCESS_FLAGS(0x0100)) {
            Ok(desk) => {
                let _ = CloseDesktop(desk);
            }
            Err(_) => {
                is_uac = true;
            }
        }
        // consent.exe process
        if has_process_named("consent") {
            is_uac = true;
        }

        // foreground / target
        let fg = GetForegroundWindow();
        let target = if target_hwnd > 0 {
            windows::Win32::Foundation::HWND(target_hwnd as *mut _)
        } else {
            fg
        };

        let mut is_hung = false;
        if !target.0.is_null() {
            is_hung = IsHungAppWindow(target).as_bool();
        }

        // process info
        let mut proc_info = serde_json::Map::new();
        if !target.0.is_null() {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(target, Some(&mut pid));
            if pid > 0 {
                if let Some(info) = process_info(pid, is_hung) {
                    proc_info = info;
                }
            }
        }

        if is_uac && auto_alert {
            // play exclamation sound
            use windows::Win32::System::Diagnostics::Debug::MessageBeep;
            use windows::Win32::UI::WindowsAndMessaging::MB_ICONEXCLAMATION;
            let _ = MessageBeep(MB_ICONEXCLAMATION);
        }

        let message = if is_uac {
            "Windows UAC / Administrator elevation prompt is active on screen"
        } else if is_hung {
            "Target application is not responding"
        } else {
            "Application is healthy and responding"
        };

        let res = json!({
            "ok": true,
            "uac_prompt_active": is_uac,
            "window_hung": is_hung,
            "process": Value::Object(proc_info),
            "message": message
        });
        res
    }
}

fn has_process_named(name: &str) -> bool {
    // snapshot processes, check name
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::Foundation::CloseHandle;
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let n = String::from_utf16_lossy(
                    &pe.szExeFile[..pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len())],
                );
                let stem = n.trim_end_matches(".exe").to_lowercase();
                if stem == name.to_lowercase() {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn process_info(pid: u32, is_hung: bool) -> Option<serde_json::Map<String, Value>> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        // name
        let mut buf = vec![0u16; 32768];
        let mut size = buf.len() as u32;
        let pw = windows::core::PWSTR(buf.as_mut_ptr());
        let name = match QueryFullProcessImageNameW(handle, windows::Win32::System::Threading::PROCESS_NAME_WIN32, pw, &mut size) {
            Ok(_) => {
                let full = String::from_utf16_lossy(&buf[..size as usize]);
                std::path::Path::new(&full)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(full)
            }
            Err(_) => String::new(),
        };
        // cpu times
        let mut creation = windows::Win32::Foundation::FILETIME::default();
        let mut exit = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        let cpu_seconds = if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).is_ok() {
            let k = filetime_to_u64(kernel);
            let u = filetime_to_u64(user);
            (k + u) as f64 / 1e7
        } else {
            0.0
        };
        // working set
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        let ws_mb = if GetProcessMemoryInfo(handle, &mut pmc, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok() {
            pmc.WorkingSetSize as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        };
        let _ = CloseHandle(handle);

        let mut m = serde_json::Map::new();
        m.insert("pid".into(), json!(pid as i64));
        m.insert("name".into(), json!(name));
        m.insert("responding".into(), json!(!is_hung));
        m.insert("cpu_seconds".into(), json!((cpu_seconds * 100.0).round() / 100.0));
        m.insert("working_set_mb".into(), json!((ws_mb * 100.0).round() / 100.0));
        Some(m)
    }
}

fn filetime_to_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

