// cu-core::macros — port de src/handlers/macros.js (grabación/persistencia; el REPLAY queda
// del hijo, que lee el mismo archivo y ejecuta los pasos con su dispatch + progreso).
//
// Formato de archivo (byte-compatible con el origen): {schema:1, name, created_at, updated_at,
// step_count, steps:[{tool,args,at}]} + pretty 2-espacios + "\n".
// PORT NOTES: slugify usa tabla latina para diacríticos (sin crate de normalización Unicode);
// el puente no agrega `__element` a los pasos grabados (enriquecimiento UIA del hijo).
use crate::incidents::{iso_now, local_app_data, now_ms, sanitize_args};
use crate::js::{js_string_of, js_truthy};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub const MACRO_SCHEMA: i64 = 1;

// --- Utilidades puras --------------------------------------------------------

fn strip_diacritic(c: char) -> char {
    match c {
        'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'é' | 'è' | 'ë' | 'ê' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'í' | 'ì' | 'ï' | 'î' | 'ī' | 'ĭ' | 'į' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'õ' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ú' | 'ù' | 'ü' | 'û' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ñ' | 'ń' | 'ň' => 'n',
        'ç' | 'ć' | 'č' => 'c',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

fn normalize_text(value: &str) -> String {
    let lowered: String = value.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut out = String::new();
    let mut in_ws = false;
    for ch in lowered.chars() {
        let mapped = strip_diacritic(ch);
        if mapped.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(mapped);
            in_ws = false;
        }
    }
    out.trim_end().to_string()
}

pub fn slugify(value: &str) -> String {
    let n = normalize_text(value);
    let mut out = String::new();
    let mut last_dash = false;
    for ch in n.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.chars().take(80).collect()
}

// --- Storage -----------------------------------------------------------------

pub fn macros_dir(dir: Option<&str>) -> PathBuf {
    if let Some(d) = dir {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(v) = std::env::var("COMPUTER_USE_MACROS_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("computer-use").join("macros");
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(home)
        .join(".agents")
        .join("computer-use")
        .join("macros")
}

fn macro_path(name: &str, dir: Option<&str>) -> PathBuf {
    macros_dir(dir).join(format!("{}.json", slugify(name)))
}

fn load_macro(name: &str, dir: Option<&str>) -> Option<Value> {
    let path = macro_path(name, dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    let mut macro_v: Value = serde_json::from_str(&raw).ok()?;
    let obj = macro_v.as_object_mut()?;
    if obj.get("schema").and_then(Value::as_i64) != Some(MACRO_SCHEMA) {
        return None;
    }
    if !obj.get("steps").map(|s| s.is_array()).unwrap_or(false) {
        return None;
    }
    let name_missing = obj
        .get("name")
        .map(|v| !js_truthy(v))
        .unwrap_or(true);
    if name_missing {
        obj.insert("name".into(), json!(slugify(name)));
    }
    Some(macro_v)
}

fn save_macro(macro_v: &Value, dir: Option<&str>) -> Result<PathBuf, String> {
    let name = slugify(macro_v.get("name").and_then(Value::as_str).unwrap_or(""));
    if name.is_empty() {
        return Err("invalid_macro_name".to_string());
    }
    let root = macros_dir(dir);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let path = root.join(format!("{name}.json"));
    let now = iso_now();
    let steps = macro_v
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("schema".into(), json!(MACRO_SCHEMA));
    out.insert("name".into(), json!(name));
    out.insert(
        "created_at".into(),
        macro_v
            .get("created_at")
            .filter(|v| js_truthy(v))
            .cloned()
            .unwrap_or(json!(now)),
    );
    out.insert(
        "updated_at".into(),
        macro_v
            .get("updated_at")
            .filter(|v| js_truthy(v))
            .cloned()
            .unwrap_or(json!(now)),
    );
    out.insert("step_count".into(), json!(steps.len()));
    out.insert("steps".into(), Value::Array(steps));
    if let Some(hc) = macro_v.get("healed_count").filter(|v| v.is_number()) {
        out.insert("healed_count".into(), hc.clone());
    }
    if let Some(lr) = macro_v.get("last_replay").filter(|v| v.is_object()) {
        out.insert("last_replay".into(), lr.clone());
    }
    let mut body = serde_json::to_string_pretty(&Value::Object(out)).map_err(|e| e.to_string())?;
    body.push('\n');
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

fn list_macros(dir: Option<&str>) -> Vec<Value> {
    let root = macros_dir(dir);
    let mut out: Vec<Value> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    let root_str = root.to_string_lossy().to_string();
    for e in entries.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        if !fname.to_lowercase().ends_with(".json") {
            continue;
        }
        let stem = &fname[..fname.len() - 5];
        let Some(mac) = load_macro(stem, Some(root_str.as_str())) else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert(
            "name".into(),
            mac.get("name").cloned().unwrap_or(json!(stem)),
        );
        let step_count = mac
            .get("steps")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        entry.insert("step_count".into(), json!(step_count));
        entry.insert(
            "created_at".into(),
            mac.get("created_at")
                .filter(|v| js_truthy(v))
                .cloned()
                .unwrap_or(Value::Null),
        );
        entry.insert(
            "updated_at".into(),
            mac.get("updated_at")
                .filter(|v| js_truthy(v))
                .cloned()
                .unwrap_or(Value::Null),
        );
        if let Some(lr) = mac.get("last_replay").filter(|v| js_truthy(v)) {
            entry.insert("last_replay".into(), lr.clone());
        }
        out.push(Value::Object(entry));
    }
    out.sort_by(|a, b| {
        let ua = a
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let ub = b
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        ub.cmp(&ua)
    });
    out
}

fn delete_macro(name: &str, dir: Option<&str>) -> bool {
    let path = macro_path(name, dir);
    if !path.exists() {
        return false;
    }
    std::fs::remove_file(&path).is_ok()
}

// --- Grabación ---------------------------------------------------------------

struct Rec {
    name: String,
    steps: Vec<Value>,
    #[allow(dead_code)]
    started_at: i64,
}

static RECORDING: Mutex<Option<Rec>> = Mutex::new(None);

pub fn is_recording() -> bool {
    match RECORDING.lock() {
        Ok(g) => g.is_some(),
        Err(e) => e.into_inner().is_some(),
    }
}

fn start_recording(name: Option<&Value>) -> Value {
    let slug = slugify(&name.map(js_string_of).unwrap_or_default());
    if slug.is_empty() {
        return json!({"ok": false, "error": "invalid_name",
            "message": "name must contain alphanumeric characters"});
    }
    let mut g = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(rec) = g.as_ref() {
        let current = rec.name.clone();
        return json!({"ok": false, "error": "already_recording", "name": current});
    }
    *g = Some(Rec {
        name: slug.clone(),
        steps: Vec::new(),
        started_at: now_ms() as i64,
    });
    json!({"ok": true, "name": slug, "recording": true})
}

// Registra un paso (llamado por el hook del puente con args ya saneados aparte:
// acá se aplica sanitizeArgs igual que el origen).
pub fn note_step(tool: &str, args: &Value) {
    let mut g = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rec) = g.as_mut() else {
        return;
    };
    if tool.trim().is_empty() {
        return;
    }
    let sanitized = sanitize_args(args);
    rec.steps.push(json!({
        "tool": tool,
        "args": sanitized,
        "at": now_ms() as i64
    }));
}

fn stop_recording(dir: Option<&str>) -> Value {
    let taken = {
        let mut g = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        match g.take() {
            None => return json!({"ok": false, "error": "not_recording"}),
            Some(rec) => (rec.name, rec.steps),
        }
    };
    let (name, steps) = taken;
    let previous = load_macro(&name, dir);
    let now = iso_now();
    let mut macro_o = Map::new();
    macro_o.insert("schema".into(), json!(MACRO_SCHEMA));
    macro_o.insert("name".into(), json!(name));
    macro_o.insert(
        "created_at".into(),
        previous
            .as_ref()
            .and_then(|p| p.get("created_at"))
            .filter(|v| js_truthy(v))
            .cloned()
            .unwrap_or(json!(now)),
    );
    macro_o.insert("updated_at".into(), json!(now));
    macro_o.insert("step_count".into(), json!(steps.len()));
    macro_o.insert("steps".into(), Value::Array(steps.clone()));
    if let Some(hc) = previous
        .as_ref()
        .and_then(|p| p.get("healed_count"))
        .filter(|v| v.is_number())
    {
        macro_o.insert("healed_count".into(), hc.clone());
    }
    if let Some(lr) = previous
        .as_ref()
        .and_then(|p| p.get("last_replay"))
        .filter(|v| js_truthy(v))
    {
        macro_o.insert("last_replay".into(), lr.clone());
    }
    match save_macro(&Value::Object(macro_o), dir) {
        Ok(path) => json!({"ok": true, "name": name, "step_count": steps.len(),
            "path": path.display().to_string()}),
        Err(e) => json!({"ok": false, "error": "save_failed", "message": e}),
    }
}

fn recording_status() -> Value {
    let g = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        None => json!({"recording": false, "step_count": 0}),
        Some(rec) => json!({"recording": true, "name": rec.name, "step_count": rec.steps.len()}),
    }
}

// --- API pública para el replay nativo (crate cu-exec) ------------------------

/// Carga un macro del directorio por defecto (usado por el replay nativo).
pub fn load_macro_pub(name: &str) -> Option<Value> {
    load_macro(name, None)
}

/// Guarda un macro (healing del replay) en el directorio por defecto.
pub fn save_macro_pub(macro_v: &Value) -> Result<PathBuf, String> {
    save_macro(macro_v, None)
}

/// Ruta del archivo de un macro (para responder `path`).
pub fn macro_path_pub(name: &str) -> PathBuf {
    macro_path(name, None)
}

// --- Tool MCP ----------------------------------------------------------------

// computer_macro_record -> (payload, isError, structuredContent).
pub fn handle_record(args: &Value) -> (Value, bool, Option<Value>) {
    let action = args
        .get("action")
        .map(js_string_of)
        .unwrap_or_default()
        .trim()
        .to_string();
    match action.as_str() {
        "start" => {
            let res = start_recording(args.get("name"));
            let is_err = res.get("ok") == Some(&Value::Bool(false));
            (res, is_err, None)
        }
        "stop" => {
            let res = stop_recording(None);
            let is_err = res.get("ok") == Some(&Value::Bool(false));
            (res, is_err, None)
        }
        "status" => (recording_status(), false, None),
        _ => {
            let shown = if action.is_empty() {
                "(empty)".to_string()
            } else {
                action
            };
            (
                json!({"ok": false, "error": "unsupported macro_record action",
                    "message": format!("unsupported macro_record action: {shown}")}),
                true,
                None,
            )
        }
    }
}

pub fn handle_list() -> (Value, bool, Option<Value>) {
    let list = list_macros(None);
    (
        Value::Array(list.clone()),
        false,
        Some(json!({"macros": list})),
    )
}

pub fn handle_delete(args: &Value) -> (Value, bool, Option<Value>) {
    let name = args.get("name").map(js_string_of).unwrap_or_default();
    let name = name.trim().to_string();
    if name.is_empty() {
        return (
            json!({"ok": false, "error": "execution_failed", "message": "name parameter is required"}),
            true,
            None,
        );
    }
    if args.get("confirm") != Some(&Value::Bool(true)) {
        return (
            json!({"ok": false, "error": "confirm_required",
                "message": "computer_macro_delete requires confirm:true",
                "elicitation": "unsupported"}),
            true,
            None,
        );
    }
    let deleted = delete_macro(&name, None);
    (
        json!({"ok": deleted, "name": slugify(&name), "deleted": deleted}),
        !deleted,
        None,
    )
}
