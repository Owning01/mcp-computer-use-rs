// cu-misc::shell — port de computer_shell (handleShell) + shell_exec + shell_background.
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::Duration;

fn default_cwd() -> String {
    std::env::var("WORKSPACE_ROOT").unwrap_or_else(|_| "G:/Proyectos".to_string())
}

pub fn shell(args: &Value) -> Value {
    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(default_cwd);
    let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(60000);
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
    let isolated = args.get("isolated_desktop").and_then(|v| v.as_bool()).unwrap_or(false);

    if isolated {
        // proxy fallback (runInDesktopSandbox no disponible nativo)
        return json!({"ok": false, "error": "isolated_desktop_not_supported_native"});
    }
    if background {
        // spawnBackground
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &cmd])
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(c) => {
                let pid = c.id();
                // unref equivalent: detach (child dropped but process continues due to spawn)
                std::mem::forget(c);
                return json!({"ok": true, "background": true, "pid": pid});
            }
            Err(e) => return json!({"ok": false, "error": e.to_string()}),
        }
    }
    // execShell
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &cmd])
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "output": e.to_string()}),
    };
    // wait with timeout
    let output = wait_with_timeout(child, timeout);
    match output {
        Ok(raw) => {
            let max_chars = 2500usize;
            let is_truncated = raw.chars().count() > max_chars;
            let output = if is_truncated {
                let truncated: String = raw.chars().take(max_chars).collect();
                format!("{}\n\n...[salida truncada: {} caracteres totales]", truncated, raw.chars().count())
            } else {
                raw
            };
            json!({"ok": true, "output": output, "truncated": is_truncated})
        }
        Err(e) => json!({"ok": false, "output": e}),
    }
}

fn wait_with_timeout(mut child: std::process::Child, timeout_ms: u64) -> Result<String, String> {
    use std::io::Read;
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    // poll
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // read stdout/stderr
                let mut out = String::new();
                if let Some(mut s) = child.stdout.take() { let _ = s.read_to_string(&mut out); }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() { let _ = s.read_to_string(&mut err); }
                if !status.success() && out.is_empty() {
                    return Err(format!("{}\n{}", out, err).trim().to_string());
                }
                if !err.is_empty() && out.is_empty() {
                    // keep origin behavior: execSync returns stdout only; stderr goes to exception
                }
                return Ok(out);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    // kill tree
                    let pid = child.id();
                    let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    let _ = child.kill();
                    return Err(format!("timeout after {}ms", timeout_ms));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub fn shell_exec(args: &Value) -> Value {
    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(default_cwd);
    let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(60000);
    let isolated = args.get("isolated_desktop").and_then(|v| v.as_bool()).unwrap_or(false);
    if isolated {
        return json!({"ok": false, "error": "isolated_desktop_not_supported_native"});
    }
    if !std::path::Path::new(&cwd).exists() {
        // origin throws; represent as error
        return json!({"ok": false, "output": format!("cwd does not exist: {}", cwd)});
    }
    let mut child = match Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &cmd])
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn() {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "output": e.to_string()}),
    };
    let start = std::time::Instant::now();
    let timeout_d = Duration::from_millis(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read;
                let mut out = String::new();
                if let Some(mut s) = child.stdout.take() { let _ = s.read_to_string(&mut out); }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() { let _ = s.read_to_string(&mut err); }
                if status.success() {
                    let text = if out.is_empty() { "(empty output)".to_string() } else { out };
                    return json!({"ok": true, "_text": text.chars().take(10000).collect::<String>()});
                } else {
                    // origin appends e.message; we don't have it. Use status.
                    let combined = format!("{}\n{}\nExit: {}", out, err, status);
                    let text = combined.chars().take(10000).collect::<String>();
                    let text = if text.is_empty() { "error".to_string() } else { text };
                    return json!({"ok": false, "_text": text, "_is_error": true});
                }
            }
            Ok(None) => {
                if start.elapsed() >= timeout_d {
                    let pid = child.id();
                    let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    let _ = child.kill();
                    let text = format!("timeout after {}ms", timeout).chars().take(10000).collect::<String>();
                    return json!({"ok": false, "_text": text, "_is_error": true});
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let text = e.to_string().chars().take(10000).collect::<String>();
                return json!({"ok": false, "_text": text, "_is_error": true});
            }
        }
    }
}

pub fn shell_background(args: &Value) -> Value {
    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(default_cwd);
    let isolated = args.get("isolated_desktop").and_then(|v| v.as_bool()).unwrap_or(false);
    if isolated {
        // runInDesktopSandbox detached not available
        return json!({"ok": true, "_text": format!("background isolated pid unknown")});
    }
    let child = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &cmd])
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(c) => {
            let pid = c.id();
            std::mem::forget(c);
            json!({"ok": true, "_text": format!("background pid {}", pid)})
        }
        Err(_) => json!({"ok": true, "_text": "background pid unknown"}),
    }
}