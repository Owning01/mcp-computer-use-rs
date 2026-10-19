// cu-core::incidents — port de src/handlers/incidents.js (F6, repo-migrate).
//
// Journal JSONL append-only: %LOCALAPPDATA%\computer-use\incidents.jsonl
// (COMPUTER_USE_INCIDENTS_DIR lo cambia; COMPUTER_USE_INCIDENTS=off lo desactiva).
// Rotacion simple a .1 al superar 5 MB (COMPUTER_USE_INCIDENTS_MAX_BYTES).
//
// PORT NOTES (vs JS):
// - Los largos de string se miden en chars (Rust) en lugar de unidades UTF-16 (JS).
// - La firma agrupable usa un matcher propio equivalente a las regex de JS (signature_of).
// - El id deriva 6 hex de un hash de (ms, pid, contador) en vez de randomBytes(3);
//   mismo formato inc_<ms>_<6hex>.
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_LIMIT: u64 = 50;
pub const MAX_LIST_LIMIT: u64 = 500;
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_STRING: usize = 200;
pub const MAX_STORED_STRING: usize = 4096; // macros.MAX_STORED_STRING
const SIGNATURE_MAX: usize = 160;
const SECRET_MARKER: &str = "__secret__";

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

// --- Entorno ---------------------------------------------------------------

pub fn incidents_enabled() -> bool {
    match std::env::var("COMPUTER_USE_INCIDENTS") {
        Ok(v) => v.trim().to_lowercase() != "off",
        Err(_) => true,
    }
}

fn local_app_data() -> PathBuf {
    if let Ok(v) = std::env::var("LOCALAPPDATA") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(home).join("AppData").join("Local")
}

pub fn incidents_dir(override_dir: Option<&str>) -> PathBuf {
    if let Some(o) = override_dir {
        if !o.is_empty() {
            return PathBuf::from(o);
        }
    }
    if let Ok(v) = std::env::var("COMPUTER_USE_INCIDENTS_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    local_app_data().join("computer-use")
}

pub fn incidents_file_path(override_dir: Option<&str>) -> PathBuf {
    incidents_dir(override_dir).join("incidents.jsonl")
}

fn max_bytes() -> u64 {
    if let Ok(v) = std::env::var("COMPUTER_USE_INCIDENTS_MAX_BYTES") {
        if let Ok(n) = v.trim().parse::<f64>() {
            if n.is_finite() && n > 0.0 {
                return n.trunc() as u64;
            }
        }
    }
    DEFAULT_MAX_BYTES
}

fn agent_label() -> String {
    match std::env::var("COMPUTER_USE_AGENT") {
        Ok(v) => {
            let label = v.trim().to_string();
            if label.is_empty() {
                format!("pid:{}", std::process::id())
            } else {
                label
            }
        }
        Err(_) => format!("pid:{}", std::process::id()),
    }
}

// --- Helpers JS -------------------------------------------------------------

fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn js_string_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => js_string_of(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

fn js_num_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn truncate_string(value: &str) -> String {
    let len = value.chars().count();
    if len <= MAX_STORED_STRING {
        value.to_string()
    } else {
        let head: String = value.chars().take(MAX_STORED_STRING).collect();
        format!("{head}[truncated +{} chars]", len - MAX_STORED_STRING)
    }
}

fn clamp_limit(value: Option<&Value>) -> u64 {
    let n = value.and_then(js_num_to_f64);
    match n {
        Some(n) if n.is_finite() && n > 0.0 => (n.trunc() as i64 as u64).min(MAX_LIST_LIMIT),
        _ => DEFAULT_LIMIT,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// Hinnant: dias civiles <-> epoch days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn iso_from_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        millis
    )
}

fn iso_now() -> String {
    iso_from_ms(now_ms() as i64)
}

// Date.parse tolerante para ISO basico (YYYY-MM-DD[THH:MM[:SS[.fff]][Z|+hh:mm]]).
fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut ms = days_from_civil(y, mo, d) * 86_400_000;
    if b.len() == 10 {
        return Some(ms);
    }
    if b[10] != b'T' && b[10] != b't' && b[10] != b' ' {
        return None;
    }
    if b.len() < 16 || b[13] != b':' {
        return None;
    }
    let hh: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let mut sec = 0i64;
    let mut millis = 0i64;
    let mut idx = 16;
    if b.len() > idx && b[idx] == b':' {
        if b.len() < idx + 3 {
            return None;
        }
        sec = s.get(idx + 1..idx + 3)?.parse().ok()?;
        idx += 3;
    }
    if b.len() > idx && b[idx] == b'.' {
        let start = idx + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        let frac = s.get(start..end)?;
        let mut f = String::new();
        for c in frac.chars().take(3) {
            f.push(c);
        }
        while f.len() < 3 {
            f.push('0');
        }
        millis = f.parse().ok()?;
        idx = end;
    }
    ms += hh * 3_600_000 + mi * 60_000 + sec * 1_000 + millis;
    if idx >= b.len() {
        return Some(ms);
    }
    match b[idx] {
        b'Z' | b'z' => Some(ms),
        b'+' | b'-' => {
            let sign = if b[idx] == b'+' { 1 } else { -1 };
            let tz: i64 = if b.len() >= idx + 6 && b[idx + 3] == b':' {
                let h: i64 = s.get(idx + 1..idx + 3)?.parse().ok()?;
                let m2: i64 = s.get(idx + 4..idx + 6)?.parse().ok()?;
                h * 60 + m2
            } else if b.len() >= idx + 3 {
                let h: i64 = s.get(idx + 1..idx + 3)?.parse().ok()?;
                h * 60
            } else {
                0
            };
            Some(ms - sign * tz * 60_000)
        }
        _ => None,
    }
}

fn new_incident_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    now_ms().hash(&mut h);
    std::process::id().hash(&mut h);
    ID_COUNTER.fetch_add(1, Ordering::SeqCst).hash(&mut h);
    format!("inc_{}_{:06x}", now_ms(), h.finish() & 0xff_ffff)
}

// --- Redaccion --------------------------------------------------------------

fn secret_key(k: &str) -> bool {
    let l = k.to_lowercase();
    if l.contains("pass")
        || l.contains("secret")
        || l.contains("token")
        || l.contains("card")
        || l.contains("cvv")
        || l.contains("pin")
    {
        return true;
    }
    // api[_-]?key en cualquier posicion
    let mut from = 0;
    while let Some(rel) = l[from..].find("api") {
        let idx = from + rel;
        let rest = &l[idx + 3..];
        let rest2 = rest
            .strip_prefix('_')
            .or_else(|| rest.strip_prefix('-'))
            .unwrap_or(rest);
        if rest2.starts_with("key") {
            return true;
        }
        from = idx + 3;
    }
    false
}

// macros.sanitizeArgs (el subconjunto que usa redactArgs).
pub fn sanitize_args(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(truncate_string(s)),
        Value::Array(a) => Value::Array(a.iter().map(sanitize_args).collect()),
        Value::Object(o) => {
            let secret_kind = if o.contains_key("secret_b64") {
                Some("secret_b64")
            } else if o.get("is_secret") == Some(&Value::Bool(true)) {
                Some("text")
            } else {
                None
            };
            let mut out = Map::new();
            for (k, v) in o {
                if secret_kind == Some("secret_b64") && k == "secret_b64" {
                    let mut m = Map::new();
                    m.insert(SECRET_MARKER.to_string(), json!("secret_b64"));
                    out.insert(k.clone(), Value::Object(m));
                    continue;
                }
                if secret_kind == Some("text") && k == "text" {
                    let mut m = Map::new();
                    m.insert(SECRET_MARKER.to_string(), json!("text"));
                    out.insert(k.clone(), Value::Object(m));
                    continue;
                }
                out.insert(k.clone(), sanitize_args(v));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn redact_pass(value: &Value, depth: usize) -> Value {
    if let Value::String(s) = value {
        let len = s.chars().count();
        if len > MAX_STRING {
            let head: String = s.chars().take(MAX_STRING).collect();
            return json!(format!("{head}…(+{})", len - MAX_STRING));
        }
        return value.clone();
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => return value.clone(),
        _ => {}
    }
    if depth > 8 {
        return json!("[depth]");
    }
    if let Value::Array(a) = value {
        return Value::Array(a.iter().take(64).map(|v| redact_pass(v, depth + 1)).collect());
    }
    if let Value::Object(o) = value {
        let mut out = Map::new();
        for (k, v) in o {
            out.insert(
                k.clone(),
                if secret_key(k) {
                    json!("***")
                } else {
                    redact_pass(v, depth + 1)
                },
            );
        }
        return Value::Object(out);
    }
    value.clone()
}

pub fn redact_args(value: &Value) -> Value {
    redact_pass(&sanitize_args(value), 0)
}

// --- Clasificacion ----------------------------------------------------------

fn word_in(hay: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(rel) = hay[from..].find(word) {
        let idx = from + rel;
        let before_ok = idx == 0 || !hay[..idx].chars().next_back().map(is_word).unwrap_or(false);
        let after = idx + word.len();
        let after_ok = after >= hay.len() || !hay[after..].chars().next().map(is_word).unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        from = idx + 1;
    }
    false
}

fn normalize_error_input(input: &Value) -> (Option<String>, String, Option<String>) {
    match input {
        Value::Null => return (None, String::new(), None),
        Value::String(s) => return (None, s.clone(), None),
        Value::Object(_) => {}
        other => return (None, js_string_of(other), None),
    }
    let obj = match input.as_object() {
        Some(o) => o,
        None => return (None, String::new(), None),
    };
    let empty = Map::new();
    let sc = obj
        .get("structuredContent")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);

    let mut code: Option<Value> = None;
    if let Some(c) = sc.get("code") {
        if js_truthy(c) {
            code = Some(c.clone());
        }
    }
    if code.is_none() {
        if let Some(Value::String(s)) = obj.get("code") {
            code = Some(Value::String(s.clone()));
        }
    }

    let mut message: Option<Value> = None;
    for cand in [sc.get("message"), sc.get("error")] {
        match cand {
            Some(c) if js_truthy(c) => {
                message = Some(c.clone());
                break;
            }
            _ => {}
        }
    }
    if message.is_none() {
        if let Some(Value::String(s)) = obj.get("error") {
            if !s.is_empty() {
                message = Some(Value::String(s.clone()));
            }
        }
    }
    if message.is_none() {
        if let Some(Value::String(s)) = obj.get("message") {
            if !s.is_empty() {
                message = Some(Value::String(s.clone()));
            }
        }
    }

    if let Some(err) = obj.get("error").and_then(|v| v.as_object()) {
        if !message.as_ref().map(js_truthy).unwrap_or(false) {
            match err.get("message") {
                Some(em) if js_truthy(em) => message = Some(em.clone()),
                _ => message = Some(json!("")),
            }
        }
        if !code.as_ref().map(js_truthy).unwrap_or(false) {
            if let Some(ec) = err.get("code") {
                code = Some(ec.clone());
            }
        }
    }

    // content[type=text] -> intento JSON
    let mut text_block: Option<String> = None;
    if let Some(arr) = obj.get("content").and_then(|v| v.as_array()) {
        for c in arr {
            if c.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(t) = c.get("text").and_then(Value::as_str) {
                    text_block = Some(t.to_string());
                    break;
                }
            }
        }
    }
    if let Some(text) = text_block {
        let parsed = serde_json::from_str::<Value>(&text).ok();
        match parsed {
            Some(p @ Value::Object(_)) => {
                if !message.as_ref().map(js_truthy).unwrap_or(false) {
                    let pm = p
                        .get("message")
                        .filter(|v| js_truthy(v))
                        .cloned()
                        .or_else(|| p.get("error").filter(|v| js_truthy(v)).cloned());
                    if let Some(v) = pm {
                        message = Some(v);
                    }
                }
                if !code.as_ref().map(js_truthy).unwrap_or(false) {
                    if let Some(c) = p.get("code").filter(|v| js_truthy(v)) {
                        code = Some(c.clone());
                    }
                }
            }
            _ => {
                if !message.as_ref().map(js_truthy).unwrap_or(false) {
                    message = Some(Value::String(text));
                }
            }
        }
    }

    let name = obj.get("name").and_then(Value::as_str).map(|s| s.to_string());
    let message_s = match message {
        Some(m) => {
            if js_truthy(&m) {
                js_string_of(&m)
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    let code_s = match code {
        None => None,
        Some(Value::Null) => None,
        Some(v) => Some(js_string_of(&v)),
    };
    (name, message_s, code_s)
}

// Error -> clase. Orden de prioridad del origen: busy > cancelled > capture(code) >
// timeout > not_found > access_denied > blocked_browser > barrier > capture(hay) >
// invalid_args > app_error.
pub fn classify_error(input: &Value) -> &'static str {
    let (name_o, message, code_o) = normalize_error_input(input);
    let code = code_o.unwrap_or_default().to_lowercase();
    let name = name_o.unwrap_or_default().to_lowercase();
    let hay = format!("{name} {code} {message}").to_lowercase();

    if code.contains("desktop_busy")
        || code.contains("window_busy")
        || code.contains("lock_io_error")
        || hay.contains("desktop_busy")
        || hay.contains("window_busy")
    {
        "busy"
    } else if code.contains("task_cancelled")
        || code.contains("aborted")
        || code.contains("abort")
        || code.contains("cancel")
        || name == "aborterror"
        || hay.contains("abort")
        || hay.contains("cancel")
    {
        "cancelled"
    } else if code.contains("wgc") || code.contains("capture") {
        "capture_failure"
    } else if code.contains("etimedout")
        || code.contains("timeout")
        || hay.contains("timeout")
        || hay.contains("timed out")
        || hay.contains("deadline")
        || hay.contains("wait_")
        || hay.contains("no respondi")
        || hay.contains("no response")
    {
        "timeout"
    } else if hay.contains("not_found")
        || hay.contains("enoent")
        || hay.contains("not found")
        || hay.contains("no existe")
        || hay.contains("no se encontr")
        || hay.contains("no se pudo resolver")
        || hay.contains("no se pudo encontrar")
        || hay.contains("could not find")
        || hay.contains("cannot find")
        || hay.contains("unable to find")
        || hay.contains("missing")
        || hay.contains("no such")
        || word_in(&hay, "element")
        || word_in(&hay, "window")
    {
        "not_found"
    } else if hay.contains("eacces")
        || hay.contains("eperm")
        || hay.contains("access_denied")
        || hay.contains("access denied")
        || hay.contains("privilege")
        || hay.contains("uac")
        || hay.contains("elevation")
        || hay.contains("denied")
        || hay.contains("administrator")
    {
        "access_denied"
    } else if hay.contains("blocked_browser")
        || hay.contains("guard")
        || hay.contains("blocked by browser")
        || hay.contains("browser blocked")
    {
        "blocked_browser"
    } else if hay.contains("captcha")
        || hay.contains("cloudflare")
        || hay.contains("turnstile")
        || hay.contains("recaptcha")
        || hay.contains("hcaptcha")
        || hay.contains("challenge")
    {
        "barrier"
    } else if word_in(&hay, "wgc")
        || hay.contains("wgc_")
        || hay.contains("black frame")
        || hay.contains("frame negro")
        || hay.contains("uniform frame")
        || hay.contains("printwindow")
        || hay.contains("capture failed")
        || hay.contains("fallo de captura")
    {
        "capture_failure"
    } else if hay.contains("invalid")
        || hay.contains("required")
        || hay.contains("must be")
        || hay.contains("missing parameter")
        || hay.contains("bad request")
        || hay.contains("malformed")
        || hay.contains("fuera de rango")
    {
        "invalid_args"
    } else {
        "app_error"
    }
}

// --- Firma -------------------------------------------------------------------

fn is_ws(c: char) -> bool {
    c.is_whitespace()
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

// Regla 1: /(?i)[a-z]:\\[^\s"']*/g -> "<path>"   (input ya en minusculas)
fn rule_drive_path(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_alphabetic()
            && i + 2 < s.len()
            && s[i + 1] == ':'
            && s[i + 2] == '\\'
        {
            let mut j = i + 3;
            while j < s.len() && !is_ws(s[j]) && s[j] != '"' && s[j] != '\'' {
                j += 1;
            }
            out.extend("<path>".chars());
            i = j;
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Regla 2: /\\\\[^\s"']+/g -> "<path>"  (dos backslashes + cola)
fn rule_unc_path(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == '\\' && i + 1 < s.len() && s[i + 1] == '\\' {
            let mut j = i + 2;
            while j < s.len() && !is_ws(s[j]) && s[j] != '"' && s[j] != '\'' {
                j += 1;
            }
            if j > i + 2 {
                out.extend("<path>".chars());
                i = j;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Regla 3: /(^|\s)\/(?:[^\s"'\/]+\/)+[^\s"']*/g -> "$1<path>"
fn rule_unix_path(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let boundary = i == 0 || is_ws(s[i - 1]);
        if boundary && s[i] == '/' {
            let mut p = i + 1;
            let mut segments = 0usize;
            loop {
                let seg_start = p;
                while p < s.len() && !is_ws(s[p]) && s[p] != '"' && s[p] != '\'' && s[p] != '/' {
                    p += 1;
                }
                if p > seg_start && p < s.len() && s[p] == '/' {
                    segments += 1;
                    p += 1;
                } else {
                    break;
                }
            }
            if segments > 0 {
                while p < s.len() && !is_ws(s[p]) && s[p] != '"' && s[p] != '\'' {
                    p += 1;
                }
                out.extend("<path>".chars());
                i = p;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Regla 4: /\b(hwnd|handle|pid|id)\s*[:=#]?\s*\d+/gi -> "<key>:n"
fn rule_kv_ids(s: &[char]) -> Vec<char> {
    let keywords = ["hwnd", "handle", "pid", "id"];
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let boundary_before = i == 0 || !is_word(s[i - 1]);
        if boundary_before {
            let mut matched: Option<(&str, usize)> = None;
            for kw in keywords {
                let k = kw.chars().count();
                if i + k <= s.len() && s[i..i + k].iter().collect::<String>() == kw {
                    matched = Some((kw, k));
                    break;
                }
            }
            if let Some((kw, k)) = matched {
                let after = i + k;
                let boundary_after = after >= s.len() || !is_word(s[after]);
                if boundary_after {
                    let mut p = after;
                    while p < s.len() && is_ws(s[p]) {
                        p += 1;
                    }
                    if p < s.len() && (s[p] == ':' || s[p] == '=' || s[p] == '#') {
                        p += 1;
                    }
                    while p < s.len() && is_ws(s[p]) {
                        p += 1;
                    }
                    let digits_start = p;
                    while p < s.len() && s[p].is_ascii_digit() {
                        p += 1;
                    }
                    if p > digits_start {
                        out.extend(format!("{kw}:n").chars());
                        i = p;
                        continue;
                    }
                }
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Reglas 5+6: \b0x[0-9a-f]+\b y \b[0-9a-f]{8,}\b -> "<hex>"
fn rule_hex(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let boundary_before = i == 0 || !is_word(s[i - 1]);
        if boundary_before && s[i] == '0' && i + 1 < s.len() && s[i + 1] == 'x' {
            let mut j = i + 2;
            while j < s.len() && s[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > i + 2 && (j >= s.len() || !is_word(s[j])) {
                out.extend("<hex>".chars());
                i = j;
                continue;
            }
        }
        if boundary_before && s[i].is_ascii_hexdigit() {
            let mut j = i;
            while j < s.len() && s[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j - i >= 8 && (j >= s.len() || !is_word(s[j])) {
                out.extend("<hex>".chars());
                i = j;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Reglas 7+8: \d+ -> "#" y \s+ -> " " (sobre el resto)
fn collapse_digits_ws(s: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_digit() {
            out.push('#');
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }
        if is_ws(s[i]) {
            out.push(' ');
            while i < s.len() && is_ws(s[i]) {
                i += 1;
            }
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

// Firma estable para agrupar: sin digitos, hex, rutas ni hwnds.
pub fn signature_of(message: Option<&str>) -> String {
    let raw = message.unwrap_or("");
    if raw.is_empty() {
        return "(sin mensaje)".to_string();
    }
    let s: Vec<char> = raw.to_lowercase().chars().collect();
    let s = rule_drive_path(&s);
    let s = rule_unc_path(&s);
    let s = rule_unix_path(&s);
    let s = rule_kv_ids(&s);
    let s = rule_hex(&s);
    let s = collapse_digits_ws(&s);
    let joined: String = s.iter().collect();
    joined.trim().chars().take(SIGNATURE_MAX).collect()
}

// --- Persistencia ------------------------------------------------------------

fn rotate_if_needed(file: &std::path::Path) {
    if let Ok(md) = std::fs::metadata(file) {
        if md.len() > max_bytes() {
            let target = PathBuf::from(format!("{}.1", file.display()));
            let _ = std::fs::rename(file, target);
        }
    }
}

// Append silencioso: si falla el log, el tool sigue funcionando.
fn append_line(entry: &Value, dir: Option<&str>) -> bool {
    let file = incidents_file_path(dir);
    let Some(parent) = file.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    rotate_if_needed(&file);
    let mut line = match serde_json::to_string(entry) {
        Ok(l) => l,
        Err(_) => return false,
    };
    line.push('\n');
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        Ok(mut f) => f.write_all(line.as_bytes()).is_ok(),
        Err(_) => false,
    }
}

// --- Registro ----------------------------------------------------------------

fn derive_target(args: &Value) -> Option<Value> {
    let o = args.as_object()?;
    let mut target = Map::new();
    for key in ["hwnd", "window", "handle"] {
        if let Some(v) = o.get(key) {
            if v.is_null() {
                continue;
            }
            if let Value::String(s) = v {
                if s.is_empty() {
                    continue;
                }
            }
            let n = js_num_to_f64(v);
            match n {
                Some(n) if n.is_finite() => {
                    if n.fract() == 0.0 {
                        target.insert("hwnd".into(), json!(n as i64));
                    } else {
                        target.insert("hwnd".into(), json!(n));
                    }
                }
                _ => {
                    target.insert("hwnd".into(), Value::String(js_string_of(v)));
                }
            }
            break;
        }
    }
    for key in ["process", "process_name", "exe", "app"] {
        let value = o
            .get(key)
            .map(|v| if js_truthy(v) { js_string_of(v) } else { String::new() })
            .unwrap_or_default()
            .trim()
            .to_string();
        if !value.is_empty() {
            target.insert("process".into(), Value::String(value));
            break;
        }
    }
    for key in ["window_title", "title", "target_window"] {
        let value = o
            .get(key)
            .map(|v| if js_truthy(v) { js_string_of(v) } else { String::new() })
            .unwrap_or_default()
            .trim()
            .to_string();
        if !value.is_empty() {
            target.insert("window_title".into(), Value::String(value));
            break;
        }
    }
    if target.is_empty() {
        None
    } else {
        Some(Value::Object(target))
    }
}

fn is_error_result(result: &Value) -> bool {
    let Some(o) = result.as_object() else {
        return false;
    };
    if o.get("isError") == Some(&Value::Bool(true)) {
        return true;
    }
    if let Some(sc) = o.get("structuredContent").and_then(|v| v.as_object()) {
        let has_code = sc.get("code").map(js_truthy).unwrap_or(false);
        let ok_false = sc.get("ok") == Some(&Value::Bool(false));
        let has_error = sc.contains_key("error");
        if has_code && (ok_false || has_error) {
            return true;
        }
    }
    false
}

pub struct IncidentRecord<'a> {
    pub tool: &'a str,
    pub args: &'a Value,
    pub error: Option<&'a Value>,
    pub result: Option<&'a Value>,
    pub duration_ms: Option<f64>,
    pub version: &'a str,
    pub session_id: Option<&'a str>,
    pub dir: Option<&'a str>,
}

// Registra un incidente si hubo error. Devuelve la entrada o None. Nunca lanza.
pub fn record_incident(rec: IncidentRecord) -> Option<Value> {
    if !incidents_enabled() {
        return None;
    }
    let is_err = rec.error.is_some() || rec.result.map(is_error_result).unwrap_or(false);
    if !is_err {
        return None;
    }
    let source: &Value = rec.error.or(rec.result)?;
    let (name, message, code) = normalize_error_input(source);

    let mut o = Map::new();
    o.insert("type".into(), json!("incident"));
    o.insert("id".into(), json!(new_incident_id()));
    o.insert("ts".into(), json!(iso_now()));
    o.insert(
        "tool".into(),
        json!(if rec.tool.is_empty() { "unknown" } else { rec.tool }),
    );
    o.insert("args_summary".into(), redact_args(rec.args));
    let mut e = Map::new();
    e.insert("name".into(), json!(name.unwrap_or_else(|| "Error".to_string())));
    e.insert("message".into(), json!(message));
    if let Some(c) = code {
        e.insert("code".into(), json!(c));
    }
    o.insert("error".into(), Value::Object(e));
    o.insert("error_class".into(), json!(classify_error(source)));
    o.insert("signature".into(), json!(signature_of(Some(&message))));
    let dur = rec.duration_ms.filter(|d| d.is_finite()).unwrap_or(0.0);
    o.insert(
        "duration_ms".into(),
        if dur.fract() == 0.0 {
            json!(dur as i64)
        } else {
            json!(dur)
        },
    );
    o.insert("pid".into(), json!(std::process::id()));
    o.insert("agent".into(), json!(agent_label()));
    o.insert("version".into(), json!(rec.version));
    if let Some(sid) = rec.session_id {
        if !sid.is_empty() {
            o.insert("session_id".into(), json!(sid));
        }
    }
    if let Some(t) = derive_target(rec.args) {
        o.insert("target".into(), t);
    }
    o.insert("phase".into(), json!("dispatch"));
    let entry = Value::Object(o);
    append_line(&entry, rec.dir);
    Some(entry)
}

// --- Lectura -----------------------------------------------------------------

pub struct Filters<'a> {
    pub dir: Option<&'a str>,
    pub tool: Option<&'a str>,
    pub error_class: Option<&'a str>,
    pub since: Option<&'a str>,
}

pub fn read_incidents(f: &Filters) -> Vec<Value> {
    let raw = std::fs::read_to_string(incidents_file_path(f.dir)).unwrap_or_default();
    let mut incidents: Vec<Value> = Vec::new();
    let mut notes: HashMap<String, Value> = HashMap::new();
    for line in raw.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            continue; // linea corrupta: se ignora
        };
        let Some(o) = obj.as_object() else {
            continue;
        };
        let id = o.get("id").filter(|v| js_truthy(v)).map(js_string_of);
        let is_note = o.get("type").and_then(Value::as_str) == Some("note");
        if is_note {
            if let Some(id) = id {
                let res = o.get("resolution").cloned().unwrap_or(Value::Null);
                notes.insert(id, res); // gana la ultima
            }
            continue;
        }
        if id.is_some() {
            incidents.push(obj);
        }
    }
    let mut out: Vec<Value> = incidents
        .into_iter()
        .map(|inc| {
            let id = inc.get("id").map(js_string_of).unwrap_or_default();
            match notes.get(&id) {
                Some(res) => {
                    let mut o = inc.as_object().cloned().unwrap_or_default();
                    o.insert("resolution".into(), res.clone());
                    Value::Object(o)
                }
                None => inc,
            }
        })
        .collect();
    if let Some(t) = f.tool {
        out.retain(|inc| inc.get("tool").and_then(Value::as_str) == Some(t));
    }
    if let Some(ec) = f.error_class {
        out.retain(|inc| inc.get("error_class").and_then(Value::as_str) == Some(ec));
    }
    if let Some(since) = f.since {
        if !since.is_empty() {
            if let Some(t) = parse_iso_ms(since) {
                out.retain(|inc| {
                    inc.get("ts")
                        .and_then(Value::as_str)
                        .and_then(parse_iso_ms)
                        .map(|ts| ts >= t)
                        .unwrap_or(false)
                });
            }
        }
    }
    out
}

fn strip_stack(inc: &Value) -> Value {
    let mut o = inc.as_object().cloned().unwrap_or_default();
    o.remove("stack");
    Value::Object(o)
}

pub fn list_incidents(f: &Filters, limit: Option<&Value>) -> Vec<Value> {
    let lim = clamp_limit(limit) as usize;
    let all = read_incidents(f);
    let start = all.len().saturating_sub(lim);
    all[start..].iter().rev().map(strip_stack).collect()
}

pub fn get_incident(id: &str, dir: Option<&str>) -> Option<Value> {
    if id.is_empty() {
        return None;
    }
    let f = Filters {
        dir,
        tool: None,
        error_class: None,
        since: None,
    };
    read_incidents(&f)
        .into_iter()
        .find(|inc| inc.get("id").map(js_string_of).as_deref() == Some(id))
}

fn groups_table(groups: &[Value]) -> String {
    let mut lines = vec![
        "| count | resueltos | tool | error_class | firma | último seen |".to_string(),
        "|---:|---:|---|---|---|---|".to_string(),
    ];
    if groups.is_empty() {
        lines.push("| 0 | 0 | (sin incidentes) | - | - | - |".to_string());
        return lines.join("\n");
    }
    for group in groups {
        let count = group.get("count").map(js_string_of).unwrap_or_default();
        let resolved = group.get("resolved").map(js_string_of).unwrap_or_default();
        let tool = group.get("tool").map(js_string_of).unwrap_or_default();
        let ecls = group.get("error_class").map(js_string_of).unwrap_or_default();
        let signature = group
            .get("signature")
            .map(js_string_of)
            .unwrap_or_default()
            .replace('|', "\\|");
        let last_seen = group.get("last_seen").map(js_string_of).unwrap_or_default();
        lines.push(format!(
            "| {count} | {resolved} | {tool} | {ecls} | `{signature}` | {last_seen} |"
        ));
    }
    lines.join("\n")
}

pub fn summarize(f: &Filters) -> Value {
    let all = read_incidents(f);
    let total = all.len();
    let mut groups: Vec<Value> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for inc in &all {
        let tool = inc.get("tool").map(js_string_of).unwrap_or_default();
        let ecls = inc.get("error_class").map(js_string_of).unwrap_or_default();
        let sig = inc.get("signature").map(js_string_of).unwrap_or_default();
        let key = format!("{tool}::{ecls}::{sig}");
        let ts = inc.get("ts").map(js_string_of).unwrap_or_default();
        let id = inc.get("id").map(js_string_of).unwrap_or_default();
        let msg_v = inc
            .get("error")
            .and_then(|e| e.get("message"))
            .filter(|v| js_truthy(v))
            .cloned()
            .unwrap_or(json!(""));
        let resolved = inc.get("resolution").map(js_truthy).unwrap_or(false);
        match index.get(&key) {
            None => {
                index.insert(key, groups.len());
                let mut g = Map::new();
                g.insert("tool".into(), json!(tool));
                g.insert("error_class".into(), json!(ecls));
                g.insert("signature".into(), json!(sig));
                g.insert("count".into(), json!(1));
                g.insert("resolved".into(), json!(if resolved { 1 } else { 0 }));
                g.insert("first_seen".into(), json!(ts));
                g.insert("last_seen".into(), json!(ts));
                g.insert("last_id".into(), json!(id));
                g.insert("example".into(), msg_v);
                groups.push(Value::Object(g));
            }
            Some(&gi) => {
                if let Some(g) = groups[gi].as_object_mut() {
                    let c = g.get("count").and_then(Value::as_u64).unwrap_or(0) + 1;
                    g.insert("count".into(), json!(c));
                    g.insert("last_seen".into(), json!(ts));
                    g.insert("last_id".into(), json!(id));
                    if js_truthy(&msg_v) {
                        g.insert("example".into(), msg_v);
                    }
                    if resolved {
                        let r = g.get("resolved").and_then(Value::as_u64).unwrap_or(0) + 1;
                        g.insert("resolved".into(), json!(r));
                    }
                }
            }
        }
    }
    groups.sort_by(|a, b| {
        let ca = a.get("count").and_then(Value::as_i64).unwrap_or(0);
        let cb = b.get("count").and_then(Value::as_i64).unwrap_or(0);
        cb.cmp(&ca).then_with(|| {
            let la = a.get("last_seen").map(js_string_of).unwrap_or_default();
            let lb = b.get("last_seen").map(js_string_of).unwrap_or_default();
            lb.cmp(&la)
        })
    });
    let table = groups_table(&groups);
    json!({ "ok": true, "total": total, "groups": groups, "table_markdown": table })
}

pub fn export_markdown(f: &Filters, limit: Option<&Value>) -> String {
    let summary = summarize(f);
    let lim_val = match limit {
        Some(v) => v.clone(),
        None => json!(20),
    };
    let recent = list_incidents(f, Some(&lim_val));
    let total = summary.get("total").and_then(Value::as_u64).unwrap_or(0);
    let groups_len = summary
        .get("groups")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let table = summary
        .get("table_markdown")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut lines: Vec<String> = Vec::new();
    lines.push("# Incidentes computer-use — reporte RSI".to_string());
    lines.push(String::new());
    lines.push(format!(
        "Generado: {} · agente: {} · pid: {}",
        iso_now(),
        agent_label(),
        std::process::id()
    ));
    lines.push(format!(
        "Total incidentes: {total} · grupos: {groups_len} · errores recientes incluidos: {}",
        recent.len()
    ));
    lines.push(String::new());
    lines.push("## Tabla agrupada (tool · clase · firma)".to_string());
    lines.push(String::new());
    lines.push(table);
    lines.push(String::new());
    lines.push(format!("## Últimos {} errores (redactados)", recent.len()));
    lines.push(String::new());
    for inc in &recent {
        let id = inc.get("id").map(js_string_of).unwrap_or_default();
        let tool = inc.get("tool").map(js_string_of).unwrap_or_default();
        let ecls = inc.get("error_class").map(js_string_of).unwrap_or_default();
        let ts = inc.get("ts").map(js_string_of).unwrap_or_default();
        let message = inc
            .get("error")
            .and_then(|e| e.get("message"))
            .map(js_string_of)
            .unwrap_or_default();
        let message400: String = message.chars().take(400).collect();
        lines.push(format!("- `{id}` **{tool}** [{ecls}] {ts} — {message400}"));
        if let Some(target) = inc.get("target").filter(|v| js_truthy(v)) {
            if let Ok(t) = serde_json::to_string(target) {
                lines.push(format!("  target: {t}"));
            }
        }
        if let Some(res) = inc.get("resolution").filter(|v| js_truthy(v)) {
            let note = res.get("note").map(js_string_of).unwrap_or_default();
            let mut line = format!("  resolución: {note}");
            if let Some(fix) = res.get("fix").filter(|v| js_truthy(v)) {
                line.push_str(&format!(" · fix: {}", js_string_of(fix)));
            }
            lines.push(line);
        }
    }
    lines.push(String::new());
    lines.push("## Cómo cerrar el ciclo RSI".to_string());
    lines.push("1. Elegí el grupo con más `count` y diagnosticalo con `computer_incidents {action:'get', id:'<last_id>'}`.".to_string());
    lines.push("2. Proponé/aplicá el fix en el handler correspondiente.".to_string());
    lines.push("3. Anotá el resultado: `computer_incidents {action:'note', id, note:'...', fix:'...'}`.".to_string());
    lines.push("4. Verificá que el grupo deja de crecer (`action:'summary'`).".to_string());
    lines.join("\n")
}

// --- Mutaciones --------------------------------------------------------------

pub fn note_incident(
    id: Option<&Value>,
    note: Option<&Value>,
    fix: Option<&Value>,
    dir: Option<&str>,
) -> Value {
    let id_s = match id.filter(|v| js_truthy(v)) {
        Some(v) => js_string_of(v),
        None => return json!({"ok": false, "error": "id_required"}),
    };
    let f = Filters {
        dir,
        tool: None,
        error_class: None,
        since: None,
    };
    let exists = read_incidents(&f)
        .iter()
        .any(|inc| inc.get("id").map(js_string_of).as_deref() == Some(id_s.as_str()));
    if !exists {
        return json!({"ok": false, "error": "incident_not_found", "id": id_s});
    }
    let note_s = note
        .map(|v| if js_truthy(v) { js_string_of(v) } else { String::new() })
        .unwrap_or_default();
    let fix_v: Value = match fix {
        None => Value::Null,
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) if s.is_empty() => Value::Null,
        Some(v) => Value::String(js_string_of(v)),
    };
    let resolution = json!({"note": note_s, "fix": fix_v, "at": iso_now()});
    let entry = json!({"type": "note", "id": id_s, "resolution": resolution.clone()});
    append_line(&entry, dir);
    json!({"ok": true, "id": id_s, "resolution": resolution})
}

pub fn clear_incidents(confirm: &Value, dir: Option<&str>) -> Value {
    if confirm != &Value::Bool(true) {
        return json!({"ok": false, "error": "confirm_required",
            "message": "computer_incidents clear requires confirm:true"});
    }
    let file = incidents_file_path(dir);
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, "")
    })();
    match result {
        Ok(_) => json!({"ok": true, "cleared": true, "path": file.display().to_string()}),
        Err(e) => json!({"ok": false, "error": "clear_failed", "message": e.to_string()}),
    }
}

// --- Tool MCP computer_incidents ---------------------------------------------

pub fn handle_tool(args: &Value) -> (Value, bool) {
    let action = match args.get("action") {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(other) => js_string_of(other).trim().to_lowercase(),
        None => "list".to_string(),
    };
    let tool = args.get("tool").and_then(Value::as_str);
    let error_class = args.get("error_class").and_then(Value::as_str);
    let since = args.get("since").and_then(Value::as_str);
    let limit = args.get("limit");
    let base = Filters {
        dir: None,
        tool,
        error_class,
        since,
    };
    match action.as_str() {
        "list" => {
            let incidents = list_incidents(&base, limit);
            let count = incidents.len();
            (
                json!({"ok": true, "count": count, "incidents": incidents}),
                false,
            )
        }
        "get" => {
            let id = args.get("id").filter(|v| js_truthy(v));
            let Some(id_v) = id else {
                return (
                    json!({"ok": false, "error": "id_required", "message": "get requires id"}),
                    true,
                );
            };
            let id_s = js_string_of(id_v);
            match get_incident(&id_s, None) {
                Some(inc) => (json!({"ok": true, "incident": inc}), false),
                None => (json!({"ok": false, "error": "not_found", "id": id_s}), true),
            }
        }
        "summary" => (summarize(&base), false),
        "export" => {
            let md = export_markdown(&base, limit);
            (json!({"ok": true, "markdown": md}), false)
        }
        "note" => {
            let res = note_incident(args.get("id"), args.get("note"), args.get("fix"), None);
            let is_err = res.get("ok") == Some(&Value::Bool(false));
            (res, is_err)
        }
        "clear" => {
            let confirm = args.get("confirm").cloned().unwrap_or(Value::Null);
            let res = clear_incidents(&confirm, None);
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
