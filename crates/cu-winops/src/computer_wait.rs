// PORT NOTE: computer_wait unificado. stable branch del origen referencia nativeWaitStable
// (no definido) -> se devuelve not_implemented. download portado con std::fs.
use crate::wait_for::uia_exists;
use cu_core::out::ToolOut;
use serde_json::json;
use std::fs;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn computer_wait(args: &serde_json::Value) -> ToolOut {
    let cond = args.get("condition").and_then(|v| v.as_str()).unwrap_or("stable").trim().to_string();
    let timeout = args.get("timeout_ms").and_then(|v| v.as_i64()).unwrap_or(10000);
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let disappear = args.get("disappear").and_then(|v| v.as_bool()).unwrap_or(false);
    let _target_window = args.get("target_window").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

    if cond == "time" {
        let waited = timeout.min(60000);
        thread::sleep(Duration::from_millis(waited as u64));
        return ToolOut::Json(json!({"ok": true, "condition": "time", "waited_ms": timeout}), false);
    }

    if cond == "download" {
        let dir = std::env::var("USERPROFILE").unwrap_or_default() + "\\Downloads";
        // args no trae download_dir en computer_wait; origin usa default.
        let res = wait_download(timeout, &dir, "");
        let is_err = !res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        return ToolOut::Json(res, is_err);
    }

    if cond == "stable" {
        // origin referencia nativeWaitStable (no definido) -> not_implemented
        return ToolOut::Json(json!({"ok": false, "condition": "stable", "error": "not_implemented"}), true);
    }

    if cond == "text" {
        if text.is_empty() {
            return ToolOut::Json(json!({"ok": false, "error": "text parameter is required for condition='text'"}), true);
        }
        let start = Instant::now();
        let t_timeout = Duration::from_millis(timeout as u64);
        while start.elapsed() < t_timeout {
            let mut found = false;
            // UIA check
            if uia_exists(None, &text, "") {
                found = true;
            }
            if !disappear && found {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(json!({"ok": true, "condition": "text", "found": true, "elapsed_ms": elapsed}), false);
            }
            if disappear && !found {
                let elapsed = start.elapsed().as_millis() as u64;
                return ToolOut::Json(json!({"ok": true, "condition": "text", "disappeared": true, "elapsed_ms": elapsed}), false);
            }
            thread::sleep(Duration::from_millis(200));
        }
        let elapsed = start.elapsed().as_millis() as u64;
        return ToolOut::Json(json!({"ok": false, "condition": "text", "error": "timeout", "elapsed_ms": elapsed}), true);
    }

    ToolOut::Json(json!({"ok": true, "condition": cond}), false)
}

fn wait_download(timeout_seconds: i64, dir: &str, ext_filter: &str) -> serde_json::Value {
    use std::path::Path;
    if !Path::new(dir).exists() {
        return json!({"ok": false, "error": "download_dir_not_found", "dir": dir});
    }
    let timeout = if timeout_seconds > 0 { timeout_seconds } else { 30 };
    let start_time = SystemTime::now() - Duration::from_secs(5);
    let sw_start = Instant::now();
    let mut found_path: Option<std::path::PathBuf> = None;

    while sw_start.elapsed().as_secs_f64() < timeout as f64 {
        // collect files
        let mut active = 0usize;
        let mut completed: Vec<(std::path::PathBuf, SystemTime)> = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let meta = match p.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                if modified < start_time {
                    continue;
                }
                let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                let is_tmp = matches!(ext.as_str(), "crdownload" | "tmp" | "part" | "download") || name.contains(".tmp");
                if is_tmp {
                    active += 1;
                } else {
                    completed.push((p, modified));
                }
            }
        }
        completed.sort_by(|a, b| b.1.cmp(&a.1));
        if !completed.is_empty() && active == 0 {
            if !ext_filter.is_empty() {
                let ef = ext_filter.trim_start_matches('.');
                if let Some(m) = completed.iter().find(|(p, _)| {
                    let e = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    e == ef || e == format!(".{}", ef)
                }) {
                    found_path = Some(m.0.clone());
                    break;
                }
            } else {
                found_path = Some(completed[0].0.clone());
                break;
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    if let Some(fp) = found_path {
        thread::sleep(Duration::from_millis(150));
        let meta = fp.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let ext = fp.extension().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let created_at = meta
            .and_then(|m| m.created().ok())
            .map(|t| {
                let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
                format!("{}", dur.as_secs())
            })
            .unwrap_or_default();
        let elapsed = sw_start.elapsed().as_secs_f64();
        json!({
            "ok": true,
            "filename": fp.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            "path": fp.to_string_lossy(),
            "size_bytes": size,
            "extension": ext,
            "created_at": created_at,
            "sha256": "",
            "elapsed_seconds": (elapsed * 100.0).round() / 100.0
        })
    } else {
        let elapsed = sw_start.elapsed().as_secs_f64();
        json!({
            "ok": false,
            "timeout": true,
            "elapsed_seconds": (elapsed * 100.0).round() / 100.0,
            "message": format!("No completed download found in {} within {} seconds", dir, timeout)
        })
    }
}

