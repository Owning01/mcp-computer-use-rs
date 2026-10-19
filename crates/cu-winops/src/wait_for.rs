// PORT NOTE: computer_wait_for portado. name/automation_id usa UIA minimo (existencia).
// Se omite el fallback top-level UIA completo; se usa cu_native::list_windows + process name.
use crate::helpers::{alias_terms, contains_ci, process_name};
use cu_core::out::ToolOut;
use serde_json::json;
use std::time::{Duration, Instant};

pub fn wait_for(args: &serde_json::Value) -> ToolOut {
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or(5000)
        .clamp(500, 30000);
    let interval_ms = args
        .get("interval_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or(250)
        .clamp(50, 2000);

    let hwnd_want = args.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let modal_dialog = args.get("modal_dialog").and_then(|v| v.as_bool()).unwrap_or(false);
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let automation_id = args
        .get("automation_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms as u64);

    while start.elapsed() < timeout {
        // hwnd branch
        if hwnd_want > 0 {
            let active = cu_native::system::active_window();
            let ahwnd = active.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
            let owner = active.get("owner_hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
            if ahwnd == hwnd_want || owner == hwnd_want {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(
                    json!({"found": true, "elapsed_ms": elapsed, "active_window": active}),
                    false,
                );
            }
        }

        // title branch
        if !title.is_empty() {
            let terms = alias_terms(&title);
            let active = cu_native::system::active_window();
            let active_title = active.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let modal_title = active.get("modal_title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // process name from pid? active has pid; get name
            let pid = active.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let pname = if pid > 0 { process_name(pid) } else { String::new() };

            let matches = |t: &str, p: &str| -> bool {
                let str_t = t.to_lowercase();
                let str_p = p.to_lowercase();
                terms.iter().any(|term| {
                    let tl = term.to_lowercase();
                    str_t.contains(&tl) || str_p.contains(&tl)
                })
            };

            if matches(&active_title, &pname) || matches(&modal_title, &pname) {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(
                    json!({"found": true, "elapsed_ms": elapsed, "active_window": active}),
                    false,
                );
            }

            // top-level window existence check
            if let Some(found_win) = find_top_window(&terms) {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(
                    json!({"found": true, "elapsed_ms": elapsed, "window": found_win}),
                    false,
                );
            }
        }

        // modal_dialog branch
        if modal_dialog {
            let active = cu_native::system::active_window();
            let is_modal = active.get("is_modal").and_then(|v| v.as_bool()).unwrap_or(false);
            if is_modal {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(
                    json!({"found": true, "modal": true, "elapsed_ms": elapsed, "modal_window": active}),
                    false,
                );
            }
        }

        // name/automation_id branch (UIA existence)
        if !name.is_empty() || !automation_id.is_empty() {
            let hwnd_opt = args.get("hwnd").and_then(|v| v.as_i64());
            if uia_exists(hwnd_opt, &name, &automation_id) {
                let elapsed = start.elapsed().as_millis() as u64;
                // return element minimal
                return ToolOut::Json(
                    json!({"found": true, "elapsed_ms": elapsed, "element": {"name": name, "automation_id": automation_id}}),
                    false,
                );
            }
        }

        std::thread::sleep(Duration::from_millis(interval_ms as u64));
    }

    let elapsed = start.elapsed().as_millis() as u64;
    ToolOut::Json(
        json!({"found": false, "timeout": true, "elapsed_ms": elapsed}),
        true,
    )
}

fn find_top_window(terms: &[String]) -> Option<serde_json::Value> {
    // search cu_native list_windows + process name
    let wins = cu_native::list_windows(None);
    for w in &wins {
        let title = &w.title;
        let pid = w.pid;
        let pname = process_name(pid);
        for t in terms {
            if contains_ci(title, t) || contains_ci(&pname, t) {
                return Some(json!({"hwnd": w.hwnd, "title": title, "process": pname, "found": true}));
            }
        }
    }
    // Get-Process fallback: enumerate processes with main window
    use crate::helpers::enum_pids;
    for pid in enum_pids() {
        if pid == 0 {
            continue;
        }
        let pname = process_name(pid);
        // need main window handle; not available without EnumWindows per pid. Check via list? already.
        // Check process name match then find any window with that pid
        let mut proc_match = false;
        for t in terms {
            if pname.to_lowercase().contains(&t.to_lowercase()) {
                proc_match = true;
                break;
            }
        }
        if !proc_match {
            continue;
        }
        // find window handle for pid
        // reuse list_windows filter by pid
        for w in cu_native::list_windows(None) {
            if w.pid == pid {
                return Some(json!({"hwnd": w.hwnd, "title": w.title, "process": pname, "found": true}));
            }
        }
    }
    None
}

pub fn uia_exists(hwnd_opt: Option<i64>, name: &str, automation_id: &str) -> bool {
    if name.is_empty() && automation_id.is_empty() {
        return false;
    }
    unsafe {
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
        // VARIANT conversion uses fully-qualified type below.
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_AutomationIdPropertyId,
            UIA_NamePropertyId,
        };
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let auto: IUIAutomation = match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
            Ok(a) => a,
            Err(_) => return false,
        };
        let root = if let Some(h) = hwnd_opt {
            if h > 0 {
                let hwnd = windows::Win32::Foundation::HWND(h as isize as *mut core::ffi::c_void);
                match auto.ElementFromHandle(hwnd) {
                    Ok(e) => e,
                    Err(_) => match auto.GetRootElement() { Ok(r) => r, Err(_) => return false },
                }
            } else {
                match auto.GetRootElement() { Ok(r) => r, Err(_) => return false }
            }
        } else {
            match auto.GetRootElement() { Ok(r) => r, Err(_) => return false }
        };

        // build condition for automation_id if present else name
        let (prop_id, val_str) = if !automation_id.is_empty() {
            (UIA_AutomationIdPropertyId, automation_id)
        } else {
            (UIA_NamePropertyId, name)
        };

        let v: windows::Win32::System::Variant::VARIANT = val_str.into();

        let cond = match auto.CreatePropertyCondition(prop_id, &v) {
            Ok(c) => c,
            Err(_) => return false,
        };
        // FindFirst
        match root.FindFirst(TreeScope_Descendants, &cond) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}
