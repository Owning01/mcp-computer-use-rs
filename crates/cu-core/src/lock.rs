// cu-core::lock — port de src/handlers/lock.js (F6, repo-migrate).
//
// Lock multi-agente cross-proceso sobre %LOCALAPPDATA%\computer-use\locks.json
// (COMPUTER_USE_LOCK_DIR lo cambia; COMPUTER_USE_LOCK=off lo desactiva), con guard
// CAS (locks.guard, O_CREAT|O_EXCL + robo por staleness) y store atomico (tmp+rename).
//
// PORT NOTES:
// - Unica llamada Win32 de cu-core: pid_alive (OpenProcess; analogo a process.kill(pid,0)).
// - Helpers compartidos (local_app_data/agent_label/iso_from_ms) viven en incidents.rs.
// - elicitation: el puente no soporta elicitation MCP -> force sin confirm devuelve
//   {confirm_required, elicitation:"unsupported"} (el smoke espera confirm_required).
// - El jitter del guard usa (now_ms() % 12) en vez de Math.random().
use crate::incidents::{agent_label, iso_from_ms, local_app_data};
use crate::js::{js_num_to_f64, js_string_of, js_truthy};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_TTL_MS: i64 = 15000;
pub const MAX_WAIT_MS: i64 = 10000;
pub const MAX_TTL_MS: i64 = 600000;
const MIN_TTL_MS: i64 = 1000;
const RENAME_RETRIES: usize = 3;
const RENAME_RETRY_MS: u64 = 30;
const GUARD_FILE: &str = "locks.guard";
const GUARD_STALE_MS_DEFAULT: i64 = 2000;
const GUARD_WAIT_MS_DEFAULT: i64 = 1500;

static HEX_COUNTER: AtomicU64 = AtomicU64::new(0);

// --- Helpers -----------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hex_chars(n: usize) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    now_ms().hash(&mut h);
    std::process::id().hash(&mut h);
    HEX_COUNTER.fetch_add(1, Ordering::SeqCst).hash(&mut h);
    let v = h.finish();
    let mask = if n >= 16 { u64::MAX } else { (1u64 << (n * 4)) - 1 };
    format!("{:0width$x}", v & mask, width = n)
}

fn clamp_i64(value: Option<&Value>, min: i64, max: i64) -> i64 {
    match value.and_then(js_num_to_f64) {
        Some(n) if n.is_finite() => (n.trunc() as i64).max(min).min(max),
        _ => min,
    }
}

fn log_line(message: &str) {
    eprintln!("[lock] {message}");
}

// --- Entorno ---------------------------------------------------------------

pub fn lock_dir(override_dir: Option<&str>) -> PathBuf {
    if let Some(o) = override_dir {
        if !o.is_empty() {
            return PathBuf::from(o);
        }
    }
    if let Ok(v) = std::env::var("COMPUTER_USE_LOCK_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    local_app_data().join("computer-use")
}

pub fn lock_file_path(override_dir: Option<&str>) -> PathBuf {
    lock_dir(override_dir).join("locks.json")
}

pub fn is_enabled() -> bool {
    match std::env::var("COMPUTER_USE_LOCK") {
        Ok(v) => v.trim().to_lowercase() != "off",
        Err(_) => true,
    }
}

pub fn default_ttl_ms() -> i64 {
    let n = std::env::var("COMPUTER_USE_LOCK_TTL_MS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok());
    match n {
        Some(n) if n.is_finite() && n > 0.0 => (n.trunc() as i64).max(MIN_TTL_MS).min(MAX_TTL_MS),
        _ => DEFAULT_TTL_MS,
    }
}

fn env_num_clamped(var: &str, min: i64, max: i64, default: i64, min_valid: f64) -> i64 {
    let n = std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok());
    match n {
        Some(n) if n.is_finite() && n >= min_valid => (n.trunc() as i64).max(min).min(max),
        _ => default,
    }
}

fn guard_stale_ms() -> i64 {
    env_num_clamped(
        "COMPUTER_USE_LOCK_GUARD_STALE_MS",
        200,
        60000,
        GUARD_STALE_MS_DEFAULT,
        200.0,
    )
}

fn guard_wait_ms() -> i64 {
    env_num_clamped(
        "COMPUTER_USE_LOCK_GUARD_WAIT_MS",
        0,
        60000,
        GUARD_WAIT_MS_DEFAULT,
        0.0,
    )
}

// --- Guard cross-proceso (CAS para el read-modify-write) ---------------------

struct Guard {
    file: PathBuf,
    token: String,
    fd: File,
}

thread_local! {
    static ACTIVE_GUARD: RefCell<Option<Guard>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Copy)]
enum GuardErr {
    Timeout,
    Lost,
    Missing,
}

fn guard_file_path(dir: Option<&str>) -> PathBuf {
    lock_dir(dir).join(GUARD_FILE)
}

fn mtime_ms(p: &Path) -> Option<i64> {
    std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn pid_alive(pid: f64) -> bool {
    if !pid.is_finite() || pid <= 0.0 {
        return false;
    }
    let pid = pid.trunc() as u32;
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false.into(), pid) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            // ACCESS_DENIED (5) = el proceso existe pero no podemos abrirlo: vivo.
            Err(e) => (e.code().0 as u32 & 0xFFFF) == 5,
        }
    }
}

fn try_create_guard(dir: Option<&str>) -> Option<Guard> {
    let file = guard_file_path(dir);
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let token = format!("{}:{}", std::process::id(), hex_chars(8));
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&file)
    {
        Ok(mut fd) => {
            let payload = json!({
                "token": token,
                "pid": std::process::id(),
                "label": agent_label(),
                "at": now_ms()
            });
            let _ = fd.write_all(payload.to_string().as_bytes());
            Some(Guard { file, token, fd })
        }
        Err(_) => None,
    }
}

fn steal_stale_guard(dir: Option<&str>) {
    let file = guard_file_path(dir);
    let Some(m1) = mtime_ms(&file) else {
        return;
    };
    if now_ms() - m1 <= guard_stale_ms() {
        return;
    }
    // Re-chequear justo antes de borrar: si cambio, hay un dueno nuevo.
    let Ok(before) = std::fs::read_to_string(&file) else {
        return;
    };
    let Some(m2) = mtime_ms(&file) else {
        return;
    };
    if now_ms() - m2 <= guard_stale_ms() {
        return;
    }
    let Ok(after) = std::fs::read_to_string(&file) else {
        return;
    };
    if after != before {
        return;
    }
    // Fail-closed: solo se roba si el pid ya no existe o el JSON es ilegible.
    let parsed = serde_json::from_str::<Value>(&before).ok();
    if let Some(p) = parsed.as_ref() {
        if let Some(pid) = p.get("pid").and_then(js_num_to_f64) {
            if pid_alive(pid) {
                return;
            }
        }
    }
    if std::fs::remove_file(&file).is_ok() {
        log_line(&format!("lock_guard_stolen pid={}", std::process::id()));
    }
}

fn close_guard(g: Guard) {
    drop(g.fd);
    if let Ok(raw) = std::fs::read_to_string(&g.file) {
        if let Ok(cur) = serde_json::from_str::<Value>(&raw) {
            if cur.get("token").and_then(Value::as_str) == Some(g.token.as_str()) {
                let _ = std::fs::remove_file(&g.file);
            }
        }
    }
}

// Fencing: antes de mutar el store, la seccion critica verifica que sigue siendo
// la duena del guard.
fn assert_guard_owner() -> Result<(), GuardErr> {
    ACTIVE_GUARD.with(|cell| {
        let b = cell.borrow();
        let Some(g) = b.as_ref() else {
            return Err(GuardErr::Missing);
        };
        let cur = std::fs::read_to_string(&g.file)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
        match cur {
            Some(v) if v.get("token").and_then(Value::as_str) == Some(g.token.as_str()) => Ok(()),
            _ => Err(GuardErr::Lost),
        }
    })
}

fn with_guard<T>(dir: Option<&str>, f: impl FnOnce() -> T) -> Result<T, GuardErr> {
    let deadline = now_ms() + guard_wait_ms();
    loop {
        if let Some(guard) = try_create_guard(dir) {
            ACTIVE_GUARD.with(|c| *c.borrow_mut() = Some(guard));
            let out = f();
            let taken = ACTIVE_GUARD.with(|c| c.borrow_mut().take());
            if let Some(g) = taken {
                close_guard(g);
            }
            return Ok(out);
        }
        steal_stale_guard(dir);
        if now_ms() >= deadline {
            return Err(GuardErr::Timeout);
        }
        let jitter = (now_ms() % 12) as u64;
        std::thread::sleep(Duration::from_millis(5 + jitter));
    }
}

// --- Scopes ------------------------------------------------------------------

pub fn normalize_scope(scope: Option<&Value>) -> String {
    let raw = match scope {
        None => "desktop".to_string(),
        Some(Value::Null) => "desktop".to_string(),
        Some(Value::String(s)) if s.is_empty() => "desktop".to_string(),
        Some(v) => js_string_of(v),
    }
    .trim()
    .to_string();
    let lower = raw.to_lowercase();
    if lower.is_empty() || lower == "desktop" {
        return "desktop".to_string();
    }
    if let Some(rest) = lower.strip_prefix("hwnd:") {
        if let Ok(n) = rest.trim().parse::<f64>() {
            if n.is_finite() && n > 0.0 {
                return format!("hwnd:{}", n.trunc() as i64);
            }
        }
        return "desktop".to_string();
    }
    if lower.starts_with("app:") {
        let exe = raw.get(4..).unwrap_or("").trim().to_lowercase();
        return if exe.is_empty() {
            "desktop".to_string()
        } else {
            format!("app:{exe}")
        };
    }
    lower
}

// desktop conflictua con todo; hwnd/app solo con desktop y con su mismo scope.
fn conflicts(a: &str, b: &str) -> bool {
    let sa = normalize_scope(Some(&Value::String(a.to_string())));
    let sb = normalize_scope(Some(&Value::String(b.to_string())));
    if sa == sb {
        return true;
    }
    sa == "desktop" || sb == "desktop"
}

// --- Store JSON atomico ------------------------------------------------------

#[derive(Default)]
struct Store {
    locks: serde_json::Map<String, Value>,
}

fn read_store(dir: Option<&str>) -> Store {
    let file = lock_file_path(dir);
    let Ok(raw) = std::fs::read_to_string(&file) else {
        return Store::default();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Store::default();
    };
    let Some(locks_v) = parsed.get("locks") else {
        return Store::default();
    };
    let Some(obj) = locks_v.as_object() else {
        return Store::default();
    };
    let mut locks = serde_json::Map::new();
    for (scope, owner) in obj {
        let is_owner = owner.is_object()
            && owner.get("id").map(|v| v.is_string()).unwrap_or(false);
        if is_owner {
            locks.insert(scope.clone(), owner.clone());
        }
    }
    Store { locks }
}

enum WriteErr {
    Guard,
    Io(String),
}

fn write_store(store: &Store, dir: Option<&str>) -> Result<(), WriteErr> {
    assert_guard_owner().map_err(|_| WriteErr::Guard)?;
    let file = lock_file_path(dir);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteErr::Io(e.to_string()))?;
    }
    let tmp = PathBuf::from(format!(
        "{}.{}.{}.tmp",
        file.display(),
        std::process::id(),
        hex_chars(6)
    ));
    let payload = json!({
        "v": 1,
        "updatedAt": now_ms(),
        "locks": Value::Object(store.locks.clone())
    });
    if let Err(e) = std::fs::write(&tmp, payload.to_string()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(WriteErr::Io(e.to_string()));
    }
    let mut last: Option<String> = None;
    for attempt in 0..RENAME_RETRIES {
        match std::fs::rename(&tmp, &file) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last = Some(e.to_string());
                if attempt < RENAME_RETRIES - 1 {
                    std::thread::sleep(Duration::from_millis(RENAME_RETRY_MS));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(WriteErr::Io(
        last.unwrap_or_else(|| "rename_failed".to_string()),
    ))
}

// --- Respuestas --------------------------------------------------------------

fn public_owner(owner: &serde_json::Map<String, Value>, now: i64) -> Value {
    let get = |k: &str| owner.get(k).cloned().unwrap_or(Value::Null);
    let started = owner
        .get("startedAt")
        .filter(|v| js_truthy(v))
        .and_then(js_num_to_f64)
        .unwrap_or(now as f64);
    let expires = owner
        .get("expiresAt")
        .filter(|v| js_truthy(v))
        .and_then(js_num_to_f64)
        .unwrap_or(now as f64);
    json!({
        "id": get("id"),
        "label": get("label"),
        "pid": get("pid"),
        "scope": get("scope"),
        "tool": get("tool"),
        "startedAt": get("startedAt"),
        "refreshedAt": get("refreshedAt"),
        "expiresAt": get("expiresAt"),
        "age_ms": (now as f64 - started).max(0.0) as i64,
        "expires_in_ms": (expires - now as f64).max(0.0) as i64
    })
}

fn fail_result(target: &str, blocker_scope: &str, owner_v: &Value, now: i64) -> Value {
    let owner_map = owner_v.as_object().cloned().unwrap_or_default();
    let owner = public_owner(&owner_map, now);
    let blocked_by_desktop = blocker_scope == "desktop";
    let reason = if blocked_by_desktop {
        "desktop_busy"
    } else {
        "window_busy"
    };
    let where_txt = if blocked_by_desktop {
        "el desktop".to_string()
    } else {
        format!("la ventana ({blocker_scope})")
    };
    let until = owner
        .get("expiresAt")
        .and_then(js_num_to_f64)
        .map(|ms| iso_from_ms(ms as i64))
        .unwrap_or_else(|| "?".to_string());
    let expires_in_ms = owner
        .get("expires_in_ms")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let hint = format!(
        "Otro agente ({}, pid {}) está usando {} con '{}' hasta {} (~{}s). Reintentá con wait_ms o liberá con computer_lock {{action:'force', scope:'{}', confirm:true}}.",
        owner.get("label").map(js_string_of).unwrap_or_default(),
        owner.get("pid").map(js_string_of).unwrap_or_default(),
        where_txt,
        owner.get("tool").map(js_string_of).unwrap_or_default(),
        until,
        (expires_in_ms as f64 / 1000.0).ceil() as i64,
        blocker_scope
    );
    json!({
        "ok": false,
        "reason": reason,
        "blocker_scope": blocker_scope,
        "target_scope": target,
        "owner": owner,
        "expires_in_ms": expires_in_ms,
        "hint": hint
    })
}

fn contention_result(target: &str) -> Value {
    json!({
        "ok": false,
        "reason": "lock_contention",
        "target_scope": target,
        "hint": "El guard de locks.json estuvo ocupado o se perdió (otro proceso lo retiene); reintentá en unos ms. Si quedó huérfano con pid vivo, borrá locks.guard a mano."
    })
}

// --- API ---------------------------------------------------------------------

enum Attempt {
    Acquired(Value),
    Busy(Value),
    Contention,
    IoError(Value),
}

fn acquire_once(target: &str, tool: &str, ttl: i64, dir: Option<&str>) -> Attempt {
    let now = now_ms();
    let mut store = read_store(dir);
    let mut stale: Vec<String> = Vec::new();
    for (s, owner) in store.locks.iter() {
        let is_stale = owner
            .get("expiresAt")
            .and_then(js_num_to_f64)
            .map(|e| e < now as f64)
            .unwrap_or(false);
        if is_stale {
            stale.push(s.clone());
        }
    }
    for s in &stale {
        store.locks.remove(s);
    }
    if !stale.is_empty() {
        log_line(&format!(
            "lock_stolen_stale {} (expirado; robado por {})",
            stale.join(","),
            agent_label()
        ));
        let _ = write_store(&store, dir);
    }

    let mut blocker: Option<(String, Value)> = None;
    for (s, owner) in store.locks.iter() {
        if conflicts(target, s) {
            blocker = Some((s.clone(), owner.clone()));
            break;
        }
    }
    if let Some((bs, owner_v)) = blocker {
        return Attempt::Busy(fail_result(target, &bs, &owner_v, now));
    }

    let owner = json!({
        "id": format!("{}:{}", agent_label(), hex_chars(6)),
        "label": agent_label(),
        "pid": std::process::id(),
        "scope": target,
        "tool": tool,
        "startedAt": now,
        "refreshedAt": now,
        "expiresAt": now + ttl
    });
    store.locks.insert(target.to_string(), owner.clone());
    match write_store(&store, dir) {
        Ok(_) => Attempt::Acquired(json!({"ok": true, "scope": target, "owner": owner, "ttl_ms": ttl})),
        Err(WriteErr::Guard) => Attempt::Contention,
        Err(WriteErr::Io(msg)) => {
            log_line(&format!("lock_io_error scope={target} {msg}"));
            Attempt::IoError(json!({
                "ok": false,
                "reason": "lock_io_error",
                "target_scope": target,
                "error": msg,
                "hint": "No se pudo persistir locks.json; revisá COMPUTER_USE_LOCK_DIR."
            }))
        }
    }
}

pub fn acquire(
    scope: Option<&Value>,
    tool: &str,
    ttl_ms: Option<&Value>,
    wait_ms: Option<&Value>,
    dir: Option<&str>,
) -> Value {
    if !is_enabled() {
        let target = normalize_scope(scope);
        return json!({"ok": true, "disabled": true, "owner": Value::Null, "scope": target});
    }
    let target = normalize_scope(scope);
    let ttl = match ttl_ms {
        Some(v) if !v.is_null() => clamp_i64(Some(v), MIN_TTL_MS, MAX_TTL_MS),
        _ => default_ttl_ms(),
    };
    let deadline = now_ms() + clamp_i64(wait_ms, 0, MAX_WAIT_MS);

    loop {
        let attempt = match with_guard(dir, || acquire_once(&target, tool, ttl, dir)) {
            Ok(a) => a,
            Err(_) => return contention_result(&target),
        };
        match attempt {
            Attempt::Acquired(v) => return v,
            Attempt::Contention => return contention_result(&target),
            Attempt::IoError(v) => return v,
            Attempt::Busy(v) => {
                if now_ms() >= deadline {
                    return v;
                }
                let remaining = deadline - now_ms();
                let sleep_ms = remaining.clamp(50, 250) as u64;
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
        }
    }
}

pub fn release(owner_id: Option<&str>, scope: Option<&str>, dir: Option<&str>) -> Value {
    if !is_enabled() {
        return json!({"ok": true, "disabled": true, "released": false});
    }
    let Some(owner_id) = owner_id.filter(|s| !s.is_empty()) else {
        return json!({"ok": false, "reason": "owner_required", "released": false});
    };
    let wanted = match scope {
        None => None,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(normalize_scope(Some(&Value::String(s.to_string())))),
    };
    let out = with_guard(dir, || {
        let mut store = read_store(dir);
        let mut released = false;
        let keys: Vec<String> = store.locks.keys().cloned().collect();
        for s in keys {
            if let Some(w) = &wanted {
                if &s != w {
                    continue;
                }
            }
            let matches = store
                .locks
                .get(&s)
                .and_then(|o| o.get("id"))
                .map(js_string_of)
                .as_deref()
                == Some(owner_id);
            if matches {
                store.locks.remove(&s);
                released = true;
            }
        }
        if released {
            if let Err(e) = write_store(&store, dir) {
                let msg = match e {
                    WriteErr::Io(m) => m,
                    WriteErr::Guard => "lock_guard_lost".to_string(),
                };
                log_line(&format!("lock_release_write_error {msg}"));
                return json!({"ok": false, "released": false, "error": msg});
            }
        }
        json!({"ok": true, "released": released})
    });
    match out {
        Ok(v) => v,
        Err(_) => json!({
            "ok": false,
            "reason": "lock_contention",
            "released": false,
            "hint": "Guard de locks.json ocupado o perdido; reintentá."
        }),
    }
}

pub fn extend(owner_id: &str, ttl_ms: Option<&Value>, dir: Option<&str>) -> Value {
    if !is_enabled() {
        return json!({"ok": true, "disabled": true, "extended": false});
    }
    let ttl = match ttl_ms {
        Some(v) if !v.is_null() => clamp_i64(Some(v), MIN_TTL_MS, MAX_TTL_MS),
        _ => default_ttl_ms(),
    };
    let out = with_guard(dir, || {
        let mut store = read_store(dir);
        let mut found: Option<String> = None;
        for (s, owner) in store.locks.iter_mut() {
            if owner.get("id").map(js_string_of).as_deref() == Some(owner_id) {
                let n1 = now_ms();
                let n2 = now_ms();
                if let Some(obj) = owner.as_object_mut() {
                    obj.insert("expiresAt".into(), json!(n1 + ttl));
                    obj.insert("refreshedAt".into(), json!(n2));
                }
                found = Some(s.clone());
                break;
            }
        }
        let Some(found) = found else {
            return json!({"ok": false, "reason": "not_found", "extended": false});
        };
        if let Err(e) = write_store(&store, dir) {
            let msg = match e {
                WriteErr::Io(m) => m,
                WriteErr::Guard => "lock_guard_lost".to_string(),
            };
            return json!({"ok": false, "reason": "lock_io_error", "error": msg});
        }
        json!({"ok": true, "extended": true, "scope": found, "expires_in_ms": ttl})
    });
    match out {
        Ok(v) => v,
        Err(_) => json!({
            "ok": false,
            "reason": "lock_contention",
            "extended": false,
            "hint": "Guard de locks.json ocupado o perdido; reintentá."
        }),
    }
}

pub fn force_release(scope: Option<&str>, label: Option<&str>, dir: Option<&str>) -> Value {
    let target = match scope {
        None => None,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(normalize_scope(Some(&Value::String(s.to_string())))),
    };
    let label = label.filter(|l| !l.is_empty());
    let out = with_guard(dir, || {
        let mut store = read_store(dir);
        let mut removed: Vec<Value> = Vec::new();
        let keys: Vec<String> = store.locks.keys().cloned().collect();
        for s in keys {
            if let Some(t) = &target {
                if &s != t {
                    continue;
                }
            }
            let owner_v = store.locks.get(&s).cloned().unwrap_or(Value::Null);
            if let Some(l) = label {
                if owner_v.get("label").map(js_string_of).as_deref() != Some(l) {
                    continue;
                }
            }
            store.locks.remove(&s);
            let owner_map = owner_v.as_object().cloned().unwrap_or_default();
            removed.push(json!({"scope": s, "owner": public_owner(&owner_map, now_ms())}));
        }
        if !removed.is_empty() {
            if let Err(e) = write_store(&store, dir) {
                let msg = match e {
                    WriteErr::Io(m) => m,
                    WriteErr::Guard => "lock_guard_lost".to_string(),
                };
                return json!({"ok": false, "released": false, "error": msg, "removed": []});
            }
            let scopes: Vec<String> = removed
                .iter()
                .map(|r| r.get("scope").map(js_string_of).unwrap_or_default())
                .collect();
            log_line(&format!(
                "lock_force_released {} by {}",
                scopes.join(","),
                agent_label()
            ));
        }
        let released = !removed.is_empty();
        json!({"ok": true, "released": released, "removed": removed})
    });
    match out {
        Ok(v) => v,
        Err(_) => json!({
            "ok": false,
            "reason": "lock_contention",
            "released": false,
            "removed": []
        }),
    }
}

pub fn status(dir: Option<&str>) -> Value {
    let now = now_ms();
    let store = read_store(dir);
    let mut locks: Vec<Value> = store
        .locks
        .values()
        .map(|owner| {
            let owner_map = owner.as_object().cloned().unwrap_or_default();
            let mut item = public_owner(&owner_map, now);
            let stale = owner_map
                .get("expiresAt")
                .and_then(js_num_to_f64)
                .map(|e| e < now as f64)
                .unwrap_or(false);
            if let Some(obj) = item.as_object_mut() {
                obj.insert("stale".into(), json!(stale));
            }
            item
        })
        .collect();
    locks.sort_by(|a, b| {
        let sa = a.get("startedAt").and_then(js_num_to_f64).unwrap_or(0.0);
        let sb = b.get("startedAt").and_then(js_num_to_f64).unwrap_or(0.0);
        sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
    });
    json!({
        "ok": true,
        "enabled": is_enabled(),
        "agent": agent_label(),
        "pid": std::process::id(),
        "count": locks.len(),
        "locks": locks
    })
}

// --- Tool MCP computer_lock --------------------------------------------------

pub fn handle_tool(args: &Value) -> (Value, bool) {
    let action = match args.get("action") {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(other) => js_string_of(other).trim().to_lowercase(),
        None => "status".to_string(),
    };
    match action.as_str() {
        "acquire" => {
            let default_scope = json!("desktop");
            let scope_v = args
                .get("scope")
                .filter(|v| js_truthy(v))
                .unwrap_or(&default_scope);
            let res = acquire(
                Some(scope_v),
                "computer_lock",
                args.get("ttl_ms"),
                args.get("wait_ms"),
                None,
            );
            let is_err = res.get("ok") == Some(&Value::Bool(false));
            let mut out = res.as_object().cloned().unwrap_or_default();
            out.insert("manual".into(), json!(true));
            (Value::Object(out), is_err)
        }
        "release" => {
            let store = read_store(None);
            let agent = agent_label();
            let mut released: Vec<Value> = Vec::new();
            for (s, owner) in store.locks.iter() {
                let is_mine = owner.get("pid").and_then(js_num_to_f64)
                    == Some(std::process::id() as f64)
                    && owner
                        .get("label")
                        .map(js_string_of)
                        .as_deref()
                        == Some(agent.as_str());
                if !is_mine {
                    continue;
                }
                if let Some(scope_v) = args.get("scope").filter(|v| js_truthy(v)) {
                    if normalize_scope(Some(scope_v)) != *s {
                        continue;
                    }
                }
                let owner_id = owner.get("id").map(js_string_of).unwrap_or_default();
                let res = release(Some(&owner_id), Some(s.as_str()), None);
                if res.get("released") == Some(&Value::Bool(true)) {
                    released.push(json!(s));
                }
            }
            let count = released.len();
            (json!({"ok": true, "agent": agent, "released": released, "count": count}), false)
        }
        "status" => (status(None), false),
        "extend" => {
            let store = read_store(None);
            let agent = agent_label();
            let mut extended: Vec<Value> = Vec::new();
            for (s, owner) in store.locks.iter() {
                let is_mine = owner.get("pid").and_then(js_num_to_f64)
                    == Some(std::process::id() as f64)
                    && owner
                        .get("label")
                        .map(js_string_of)
                        .as_deref()
                        == Some(agent.as_str());
                if !is_mine {
                    continue;
                }
                if let Some(scope_v) = args.get("scope").filter(|v| js_truthy(v)) {
                    if normalize_scope(Some(scope_v)) != *s {
                        continue;
                    }
                }
                let owner_id = owner.get("id").map(js_string_of).unwrap_or_default();
                let res = extend(&owner_id, args.get("ttl_ms"), None);
                if res.get("extended") == Some(&Value::Bool(true)) {
                    extended.push(json!({
                        "scope": s,
                        "expires_in_ms": res.get("expires_in_ms").cloned().unwrap_or(Value::Null)
                    }));
                }
            }
            let count = extended.len();
            (json!({"ok": true, "agent": agent, "extended": extended, "count": count}), false)
        }
        "force" => {
            if args.get("confirm") != Some(&Value::Bool(true)) {
                return (
                    json!({
                        "ok": false,
                        "error": "confirm_required",
                        "message": "computer_lock force requires confirm:true",
                        "elicitation": "unsupported"
                    }),
                    true,
                );
            }
            let scope_v = args.get("scope").filter(|v| js_truthy(v));
            let Some(scope_v) = scope_v else {
                return (
                    json!({"ok": false, "error": "scope_required",
                        "message": "force requires scope (desktop|hwnd:<n>|app:<exe>)"}),
                    true,
                );
            };
            let scope_s = js_string_of(scope_v);
            let res = force_release(Some(&scope_s), None, None);
            let is_err = res.get("ok") == Some(&Value::Bool(false));
            (res, is_err)
        }
        _ => (
            json!({"ok": false, "error": "invalid_action",
                "message": format!("unsupported action: {action}")}),
            true,
        ),
    }
}
