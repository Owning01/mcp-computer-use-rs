// computer-use2-rs — binario puente F4 (repo-migrate).
//
// Proxy MCP-a-MCP por stdio con 3 puntos de intercepcion:
//   1. initialize   -> respuesta local con la forma exacta del origen (SDK 1.30.0)
//   2. tools/list   -> metadata embebida de migration/inventory/tools-full.json (perfil agent/full)
//   3. tools/call computer_list_windows -> implementacion nativa Rust (cu-native)
// Todo lo demas (tasks/*, resources/*, progress, notifications) se proxea crudo al server Node
// del origen. Puente temporal (skill repo-migrate F4); se apaga por grupos hacia F8.
// Reglas: migration/RULEBOOK.md (ver enmienda: puente crudo por paridad byte-exacta).
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

static SHOT_COUNTER: AtomicU64 = AtomicU64::new(0);
// Última respuesta emitida por un handler nativo (para registrar incidentes como el
// origen). Solo se guarda si trae "id" (las notificaciones de progreso no cuentan).
static LAST_RESULT: Mutex<Option<Value>> = Mutex::new(None);

const ORIGIN_NODE: &str = "node";
const ORIGIN_SCRIPT: &str = "G:/Proyectos/MCP/computer-use2/src/index.js";
const ORIGIN_CWD: &str = "G:/Proyectos/MCP/computer-use2";
// El dispatcher interno (cu-exec) se activa cuando el crate está implementado; mientras
// tanto computer_batch / computer_macro_replay siguen proxieados al hijo Node.
const CU_EXEC_READY: bool = true;

const AGENT_PROFILE_TOOLS: [&str; 12] = [
    "computer_screenshot",
    "computer_act",
    "computer_observe",
    "computer_hotkey",
    "computer_window_action",
    "computer_fill_form",
    "computer_upload_file",
    "computer_login",
    "computer_pass_barrier",
    "computer_shell",
    "computer_wait",
    "computer_lock",
];

enum Src {
    Client,
    Child,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let all_tools: Vec<Value> =
        serde_json::from_str(include_str!("../../../migration/inventory/tools-full.json"))?;
    let profile = std::env::var("COMPUTER_USE_PROFILE")
        .unwrap_or_else(|_| "agent".into())
        .to_lowercase();
    let tools: Vec<Value> = if profile == "agent" || profile == "compact" {
        all_tools
            .into_iter()
            .filter(|t| {
                t["name"]
                    .as_str()
                    .map(|n| AGENT_PROFILE_TOOLS.contains(&n))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        all_tools
    };
    tracing::info!(count = tools.len(), profile = %profile, "tools cargadas");

    let disable_child = std::env::var("PARITY_DISABLE_CHILD")
        .map(|v| v == "1")
        .unwrap_or(false);
    let (mut child, mut child_in, child_out) = if disable_child {
        tracing::warn!("PARITY_DISABLE_CHILD=1: modo only-native");
        (None, None, None)
    } else {
        let (c, mut ci, co) = spawn_origin()?;
        bootstrap_child(&mut ci).await?;
        (c, ci, co)
    };

    let (tx, mut rx) = mpsc::channel::<(Src, String)>(256);

    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(l)) => {
                        if tx.send((Src::Client, l)).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            let _ = tx.send((Src::Client, "\u{0}EOF".to_string())).await;
        });
    }
    if let Some(out) = child_out {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines: Lines<BufReader<ChildStdout>> = out;
            while let Ok(Some(l)) = lines.next_line().await {
                // Las respuestas del bootstrap interno (id negativo) no se reenvian al cliente.
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    if v.get("id").and_then(Value::as_i64).is_some_and(|id| id < 0) {
                        continue;
                    }
                    // Adopta el epoch del hijo: puente e hijo comparten un único contador
                    // lógico (así el check de expected_epoch no se desincroniza).
                    if v
                        .get("id")
                        .and_then(Value::as_i64)
                        .map(|id| id >= 0)
                        .unwrap_or(false)
                    {
                        if let Some(text) =
                            v.pointer("/result/content/0/text").and_then(Value::as_str)
                        {
                            if let Ok(payload) = serde_json::from_str::<Value>(text) {
                                if let Some(ep) = payload.get("epoch").and_then(Value::as_u64) {
                                    cu_core::epoch::set(ep);
                                }
                            }
                        }
                    }
                }
                if tx.send((Src::Child, l)).await.is_err() {
                    break;
                }
            }
        });
    }

    let mut stdout = tokio::io::stdout();
    while let Some((src, line)) = rx.recv().await {
        match src {
            Src::Child => {
                respond_raw(&mut stdout, &line).await?;
            }
            Src::Client => {
                if line == "\u{0}EOF" {
                    break;
                }
                if line.trim().is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                let is_request = msg.get("id").is_some();
                match method {
                    "initialize" => {
                        let pv = msg["params"]["protocolVersion"]
                            .as_str()
                            .unwrap_or("2024-11-05");
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": msg.get("id").cloned().unwrap_or(Value::Null),
                            "result": {
                                "protocolVersion": pv,
                                "capabilities": {
                                    "tools": {},
                                    "tasks": {
                                        "list": {},
                                        "cancel": {},
                                        "requests": { "tools": { "call": {} } }
                                    }
                                },
                                "serverInfo": { "name": "computer-use", "version": "0.15.0" }
                            }
                        });
                        respond_value(&mut stdout, &resp).await?;
                    }
                    "tools/list" => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": msg.get("id").cloned().unwrap_or(Value::Null),
                            "result": { "tools": tools }
                        });
                        respond_value(&mut stdout, &resp).await?;
                    }
                    "tools/call" => {
                        let name = msg["params"]["name"].as_str().unwrap_or("");
                        let args_value = msg
                            .get("params")
                            .and_then(|p| p.get("arguments"))
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        // 1) Bloqueo de gestores de contraseñas (origen index.js:4463, ANTES del
                        // hook de macros). El puente lo aplica a lo nativo; el hijo a lo proxeado.
                        let target_win = args_value
                            .get("target_window")
                            .or_else(|| args_value.get("app"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !target_win.is_empty()
                            && cu_core::blocked::is_app_blocked(&target_win)
                        {
                            let payload = cu_core::blocked::app_blocked_payload(&target_win);
                            let text = serde_json::to_string(&payload)?;
                            let resp = json!({"jsonrpc":"2.0","id":msg.get("id").cloned().unwrap_or(Value::Null),"result":{
                                "content":[{"type":"text","text":text}],
                                "isError":true
                            }});
                            respond_value(&mut stdout, &resp).await?;
                            continue;
                        }
                        // 2) Grabación de macros (origen index.js:4471): registra TODO tool call
                        // (salvo los macro_* y los bootstrap internos de id negativo) con args
                        // saneados. PORT NOTE: el origen agrega `__element` a los clicks con
                        // id/ref (enriquecimiento UIA que vive en el proceso Node).
                        if !name.is_empty()
                            && !name.starts_with("computer_macro_")
                            && msg
                                .get("id")
                                .and_then(Value::as_i64)
                                .map(|id| id >= 0)
                                .unwrap_or(true)
                            && cu_core::macros::is_recording()
                        {
                            cu_core::macros::note_step(name, &args_value);
                        }
                        // 3) Lock multi-agente para lo que sirve el puente (lo proxeado lo toma
                        //    el hijo). Mismo plan que el origen (planLock).
                        let mut lock_owner: Option<String> = None;
                        let mut lock_scope: Option<String> = None;
                        let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;
                        if bridge_serves_native(name, &args_value) {
                            if let Some(plan) = cu_core::plan::plan_lock(name, &args_value) {
                                let acquired = cu_core::lock::acquire(
                                    Some(&Value::String(plan.scope.clone())),
                                    name,
                                    None,
                                    None,
                                    None,
                                );
                                if acquired.get("ok") != Some(&Value::Bool(true)) {
                                    let resp = busy_response(&msg, &acquired);
                                    respond_value(&mut stdout, &resp).await?;
                                    record_lock_incident(name, &args_value, &acquired);
                                    continue;
                                }
                                lock_scope = Some(plan.scope.clone());
                                lock_owner = acquired
                                    .pointer("/owner/id")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string());
                                if let Some(owner) = lock_owner.clone() {
                                    let ttl = cu_core::lock::default_ttl_ms();
                                    let interval =
                                        std::cmp::max(2000u64, (ttl.max(1) as u64) / 3);
                                    heartbeat = Some(tokio::spawn(async move {
                                        loop {
                                            tokio::time::sleep(std::time::Duration::from_millis(
                                                interval,
                                            ))
                                            .await;
                                            let ttl_v = Value::from(ttl);
                                            cu_core::lock::extend(&owner, Some(&ttl_v), None);
                                        }
                                    }));
                                }
                            }
                        }
                        // El origen registra el incidente de todo tool que falla (isError).
                        let started = std::time::Instant::now();
                        if let Ok(mut slot) = LAST_RESULT.lock() {
                            *slot = None;
                        }
                        if name == "computer_list_windows" {
                            native_list_windows(&msg, &mut stdout).await?;
                        } else if name == "computer_screenshot" {
                            native_screenshot(&msg, &mut stdout).await?;
                        } else if name == "computer_zoom" {
                            native_zoom(&msg, &mut stdout).await?;
                        } else if name == "computer_screen_info" {
                            native_screen_info(&msg, &mut stdout).await?;
                        } else if name == "computer_awake" {
                            native_awake(&msg, &mut stdout).await?;
                        } else if name == "computer_active_window" {
                            native_active_window(&msg, &mut stdout).await?;
                        } else if name == "computer_clipboard" {
                            native_clipboard(&msg, &mut stdout).await?;
                        } else if name == "computer_incidents" {
                            native_incidents(&msg, &mut stdout).await?;
                        } else if name == "computer_login"
                            && cu_core::js::flag_truthy(
                                msg.pointer("/params/arguments/list_profiles"),
                            )
                        {
                            native_login_profiles(&msg, &mut stdout).await?;
                        } else if name == "computer_lock" {
                            native_lock(&msg, &mut stdout).await?;
                        } else if name == "computer_macro_record"
                            || name == "computer_macro_list"
                            || name == "computer_macro_delete"
                        {
                            native_macros(&msg, &mut stdout).await?;
                        } else if CU_EXEC_READY
                            && (name == "computer_batch" || name == "computer_macro_replay")
                        {
                            native_exec(&msg, &mut stdout).await?;
                        } else if {
                            let null_args = Value::Null;
                            let cargs = msg
                                .get("params")
                                .and_then(|p| p.get("arguments"))
                                .unwrap_or(&null_args);
                            (cu_uia::handles(&name) && cu_uia::accepts(&name, cargs))
                                || (cu_input::handles(&name) && cu_input::accepts(&name, cargs))
                                || (cu_winops::handles(&name) && cu_winops::accepts(&name, cargs))
                                || (cu_misc::handles(&name) && cu_misc::accepts(&name, cargs))
                                || (cu_shell::handles(&name) && cu_shell::accepts(&name, cargs))
                                || (cu_ocr::handles(&name) && cu_ocr::accepts(&name, cargs))
                        } {
                            native_cluster(&name, &msg, &mut stdout).await?;
                        } else if is_request && child_in.is_none() {
                            let resp = json!({
                                "jsonrpc": "2.0",
                                "id": msg.get("id").cloned().unwrap_or(Value::Null),
                                "error": { "code": -32001, "message": "bridge child disabled (PARITY_DISABLE_CHILD=1)" }
                            });
                            respond_value(&mut stdout, &resp).await?;
                        } else {
                            forward(&mut child_in, &line).await?;
                        }
                        // 3b) Incidente por error (como recordIncidentSafe del origen).
                        let last_result = LAST_RESULT.lock().ok().and_then(|s| s.clone());
                        if let Some(resp) = last_result {
                            let is_error = resp
                                .pointer("/result/isError")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            if is_error {
                                let duration_ms = started.elapsed().as_millis() as i64;
                                record_native_incident(name, &args_value, &resp, duration_ms);
                            }
                        }
                        // 4) Liberación del lock (el origen libera siempre al terminar).
                        if let Some(hb) = heartbeat {
                            hb.abort();
                        }
                        if let (Some(owner), Some(scope)) = (lock_owner, lock_scope) {
                            cu_core::lock::release(Some(&owner), Some(&scope), None);
                        }
                    }
                    _ => {
                        if is_request && child_in.is_none() {
                            let resp = json!({
                                "jsonrpc": "2.0",
                                "id": msg.get("id").cloned().unwrap_or(Value::Null),
                                "error": { "code": -32001, "message": "bridge child disabled (PARITY_DISABLE_CHILD=1)" }
                            });
                            respond_value(&mut stdout, &resp).await?;
                        } else {
                            forward(&mut child_in, &line).await?;
                        }
                    }
                }
            }
        }
    }

    if let Some(mut c) = child.take() {
        let _ = c.kill().await;
    }
    Ok(())
}

fn ji(v: Option<&Value>, min: i64, max: i64, def: i64) -> i64 {
    let n = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match n {
        Some(f) if f.is_finite() => (f.trunc() as i64).clamp(min, max),
        _ => def,
    }
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let d = Md5::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn save_shot(bytes: &[u8], is_jpeg: bool) -> anyhow::Result<(String, String, String)> {
    let base = std::env::var("COMPUTER_USE_TMP_DIR")
        .unwrap_or_else(|_| "G:/Proyectos/MCP/computer-use2/.tmp".into());
    let dir = std::path::Path::new(&base).join("shots");
    std::fs::create_dir_all(&dir)?;
    let ext = if is_jpeg { "jpg" } else { "png" };
    let name = format!(
        "shot_{}_{}.{}",
        now_ms(),
        SHOT_COUNTER.fetch_add(1, Ordering::SeqCst) + 1,
        ext
    );
    let path = dir.join(&name);
    std::fs::write(&path, bytes)?;
    let path_str = path.to_string_lossy().replace('/', "\\");
    let uri = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
    Ok((path_str, uri, name))
}

async fn native_screenshot(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let shot_args = cu_native::capture::ShotArgs {
        width: ji(args.get("width"), 0, 100000, 0),
        format: args
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("jpeg")
            .to_string(),
        quality: ji(args.get("quality"), 10, 95, 75),
        hwnd: ji(args.get("hwnd"), 0, 281474976710655, 0),
        x: ji(args.get("x"), -1000000, 1000000, 0),
        y: ji(args.get("y"), -1000000, 1000000, 0),
        w: ji(args.get("w"), 0, 1000000, 0),
        h: ji(args.get("h"), 0, 1000000, 0),
        screen: ji(args.get("screen"), 0, 64, 0),
        capture: args
            .get("capture")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string(),
        interpolation: args
            .get("interpolation")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        wgc_timeout_ms: ji(args.get("wgc_timeout_ms"), 500, 60000, 3000),
    };
    let etag_req = args.get("etag").and_then(Value::as_str).map(str::to_string);
    let link = args.get("link").and_then(Value::as_bool).unwrap_or(false);
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    let out = match tokio::task::spawn_blocking(move || cu_native::capture::screenshot(&shot_args))
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let text = serde_json::to_string(&json!({
                "ok": false, "error": "join_error", "message": e.to_string(), "capture_method": "unknown"
            }))?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":true}});
            return respond_value(stdout, &resp).await;
        }
    };

    match out {
        cu_native::capture::ShotOut::Err { error, message, capture_method } => {
            let text = serde_json::to_string(
                &json!({"ok": false, "error": error, "message": message, "capture_method": capture_method}),
            )?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":true}});
            respond_value(stdout, &resp).await
        }
        cu_native::capture::ShotOut::Ok(ok) => {
            let epoch = cu_core::epoch::bump();
            let etag = md5_hex(&ok.bytes);
            if etag_req.as_deref() == Some(etag.as_str()) {
                let text = serde_json::to_string(&json!({
                    "unchanged": true,
                    "etag": etag,
                    "epoch": epoch,
                    "width": ok.meta.get("width").cloned().unwrap_or(Value::Null),
                    "height": ok.meta.get("height").cloned().unwrap_or(Value::Null)
                }))?;
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}});
                return respond_value(stdout, &resp).await;
            }

            // Meta con el orden exacto del wrapper del origen (index.js:4521-4539).
            let mut meta = serde_json::Map::new();
            for k in ["width", "height", "original_width", "original_height", "scale_factor", "format"] {
                if let Some(v) = ok.meta.get(k) {
                    meta.insert(k.to_string(), v.clone());
                }
            }
            meta.insert("etag".into(), json!(etag));
            meta.insert("epoch".into(), json!(epoch));
            meta.insert("unchanged".into(), json!(false));
            for k in [
                "screen_x",
                "screen_y",
                "virtual_screen",
                "capture_method",
                "capture_used",
                "printwindow_flag",
                "capture_fallback_from",
                "capture_fallback_reason",
            ] {
                if let Some(v) = ok.meta.get(k) {
                    meta.insert(k.to_string(), v.clone());
                }
            }
            let mime = if ok.is_jpeg { "image/jpeg" } else { "image/png" };

            if link {
                let (path_str, uri, name) = save_shot(&ok.bytes, ok.is_jpeg)?;
                meta.insert("path".into(), json!(path_str));
                meta.insert("mime".into(), json!(mime));
                let n = ok.bytes.len() as i64;
                meta.insert("bytes".into(), json!((n + 2) / 3 * 3));
                let text = serde_json::to_string(&Value::Object(meta))?;
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[
                    {"type":"text","text":text},
                    {"type":"resource_link","uri":uri,"name":name,"mimeType":mime}
                ]}});
                return respond_value(stdout, &resp).await;
            }

            let b64 = {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&ok.bytes)
            };
            let text = serde_json::to_string(&Value::Object(meta))?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[
                {"type":"image","data":b64,"mimeType":mime},
                {"type":"text","text":text}
            ]}});
            respond_value(stdout, &resp).await
        }
    }
}

fn clamp_scale(v: Option<&Value>) -> f64 {
    let n = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match n {
        Some(f) if f.is_finite() => f.clamp(1.0, 4.0),
        _ => 2.0,
    }
}

struct AwakeState {
    enabled: bool,
    mode: String,
    since: Option<u128>,
    reason: String,
}

static AWAKE: std::sync::Mutex<AwakeState> = std::sync::Mutex::new(AwakeState {
    enabled: false,
    mode: String::new(),
    since: None,
    reason: String::new(),
});

async fn native_screen_info(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let info = tokio::task::spawn_blocking(cu_native::screen::screen_info).await?;
    let text = serde_json::to_string(&info)?;
    let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}});
    respond_value(stdout, &resp).await
}

async fn native_awake(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let enabled = match args.get("enabled") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    };
    let mode = if args.get("mode").and_then(Value::as_str) == Some("system") {
        "system"
    } else {
        "display"
    };
    let reason = args.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
    let display = mode == "display";
    let ok = tokio::task::spawn_blocking(move || cu_native::set_keep_awake(enabled, display))
        .await
        .unwrap_or(false);
    let (awake, mode_out, message) = {
        let mut st = AWAKE.lock().unwrap_or_else(|e| e.into_inner());
        if ok {
            st.enabled = enabled;
            st.mode = if enabled { mode.to_string() } else { "none".into() };
            st.since = if enabled { Some(now_ms()) } else { None };
            st.reason = if enabled {
                if reason.is_empty() {
                    "agent_task".into()
                } else {
                    reason.clone()
                }
            } else {
                String::new()
            };
            let msg_out = if enabled {
                format!("Desktop keep-awake active (mode: {mode}, display sleep prevented)")
            } else {
                "Desktop keep-awake released (normal Windows power management restored)".to_string()
            };
            (enabled, st.mode.clone(), msg_out)
        } else {
            (
                st.enabled,
                st.mode.clone(),
                "Failed to set execution state: SetThreadExecutionState returned 0".to_string(),
            )
        }
    };
    let text = serde_json::to_string(
        &json!({ "ok": ok, "awake": awake, "mode": mode_out, "message": message }),
    )?;
    let resp = json!({"jsonrpc":"2.0","id": msg.get("id").cloned().unwrap_or(Value::Null),
        "result":{"content":[{"type":"text","text":text}],"isError":!ok}});
    respond_value(stdout, &resp).await
}

async fn native_zoom(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let zx = ji(args.get("x"), -1000000, 1000000, 0);
    let zy = ji(args.get("y"), -1000000, 1000000, 0);
    let zw = ji(args.get("w"), 1, 100000, 200);
    let zh = ji(args.get("h"), 1, 100000, 150);
    let scale = clamp_scale(args.get("scale"));
    let smooth = args.get("smooth").and_then(Value::as_bool).unwrap_or(false);
    let jpeg = args.get("format").and_then(Value::as_str) == Some("jpeg");
    let quality = ji(args.get("quality"), 10, 95, 90);
    let out_w = ((zw as f64) * scale).round().max(1.0) as i64;
    let link = args.get("link").and_then(Value::as_bool).unwrap_or(false);

    let shot_args = cu_native::capture::ShotArgs {
        width: out_w,
        format: if jpeg { "jpeg" } else { "png" }.to_string(),
        quality,
        hwnd: 0,
        x: zx,
        y: zy,
        w: zw,
        h: zh,
        screen: 0,
        capture: "gdi".to_string(),
        interpolation: if smooth { "bicubic" } else { "nearest" }.to_string(),
        wgc_timeout_ms: 3000,
    };

    let out = tokio::task::spawn_blocking(move || cu_native::capture::screenshot(&shot_args)).await?;
    match out {
        cu_native::capture::ShotOut::Err { error, message, .. } => {
            let text = serde_json::to_string(&json!({ "ok": false, "error": error, "message": message }))?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":true}});
            respond_value(stdout, &resp).await
        }
        cu_native::capture::ShotOut::Ok(ok) => {
            // PORT NOTE: el origen NO incrementa el epoch en computer_zoom (solo en screenshot).
            let epoch = cu_core::epoch::current();
            let etag = md5_hex(&ok.bytes);
            let mime = if ok.is_jpeg { "image/jpeg" } else { "image/png" };
            let mut meta = serde_json::Map::new();
            let geti = |k: &str| ok.meta.get(k).cloned().unwrap_or(Value::Null);
            meta.insert("ok".into(), json!(true));
            meta.insert(
                "region".into(),
                json!({ "x": geti("screen_x"), "y": geti("screen_y"), "w": geti("original_width"), "h": geti("original_height") }),
            );
            meta.insert("scale".into(), json!(scale));
            meta.insert("smooth".into(), json!(smooth));
            meta.insert("width".into(), json!(ok.meta.get("width").cloned().unwrap_or(Value::Null)));
            meta.insert("height".into(), json!(ok.meta.get("height").cloned().unwrap_or(Value::Null)));
            meta.insert("native_width".into(), json!(ok.meta.get("original_width").cloned().unwrap_or(Value::Null)));
            meta.insert("native_height".into(), json!(ok.meta.get("original_height").cloned().unwrap_or(Value::Null)));
            meta.insert("format".into(), json!(if ok.is_jpeg { "jpeg" } else { "png" }));
            meta.insert("etag".into(), json!(etag));
            meta.insert("epoch".into(), json!(epoch));
            meta.insert(
                "nota".into(),
                json!("las coordenadas del modelo siguen siendo físicas; el pixel central de la imagen ampliada corresponde a region.x + dx/scale (dx=columna en la imagen escalada)"),
            );

            if link {
                let (path_str, uri, name) = save_shot(&ok.bytes, ok.is_jpeg)?;
                meta.insert("path".into(), json!(path_str));
                meta.insert("mime".into(), json!(mime));
                let n = ok.bytes.len() as i64;
                meta.insert("bytes".into(), json!((n + 2) / 3 * 3));
                let text = serde_json::to_string(&Value::Object(meta))?;
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[
                    {"type":"text","text":text},
                    {"type":"resource_link","uri":uri,"name":name,"mimeType":mime}
                ]}});
                return respond_value(stdout, &resp).await;
            }
            let b64 = {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&ok.bytes)
            };
            let text = serde_json::to_string(&Value::Object(meta))?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[
                {"type":"image","data":b64,"mimeType":mime},
                {"type":"text","text":text}
            ]}});
            respond_value(stdout, &resp).await
        }
    }
}

async fn native_active_window(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let v = tokio::task::spawn_blocking(cu_native::system::active_window).await?;
    let text = serde_json::to_string(&v)?;
    let resp = json!({"jsonrpc":"2.0","id": msg.get("id").cloned().unwrap_or(Value::Null),
        "result":{"content":[{"type":"text","text":text}]}});
    respond_value(stdout, &resp).await
}

async fn native_clipboard(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");

    if action == "get" {
        // El origen devuelve el texto con trim() del pipeline PowerShell.
        let r = tokio::task::spawn_blocking(cu_native::system::clipboard_get).await?;
        return match r {
            Ok(t) => {
                let text = t.trim().to_string();
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}});
                respond_value(stdout, &resp).await
            }
            Err(e) => {
                let text = format!("ERROR: {e}");
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":true}});
                respond_value(stdout, &resp).await
            }
        };
    }

    let text_to_set = args.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let r = tokio::task::spawn_blocking(move || cu_native::system::clipboard_set(&text_to_set)).await?;
    match r {
        Ok(()) => {
            // PORT NOTE: el origen NO incrementa el epoch en computer_clipboard.
            let epoch = cu_core::epoch::current();
            let text = serde_json::to_string(
                &json!({ "ok": true, "action": "clipboard_set", "epoch": epoch }),
            )?;
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}});
            respond_value(stdout, &resp).await
        }
        Err(e) => {
            let text = format!("ERROR: {e}");
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"isError":true}});
            respond_value(stdout, &resp).await
        }
    }
}

async fn native_incidents(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let (payload, is_error) =
        tokio::task::spawn_blocking(move || cu_core::incidents::handle_tool(&args)).await?;
    let text = serde_json::to_string(&payload)?;
    let resp = json!({"jsonrpc":"2.0","id":id,"result":{
        "content":[{"type":"text","text":text}],
        "isError":is_error
    }});
    respond_value(stdout, &resp).await
}

async fn native_login_profiles(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let profiles = tokio::task::spawn_blocking(cu_core::profiles::get_browser_profiles).await?;
    let count = profiles.len();
    let text = serde_json::to_string(
        &json!({"ok": true, "count": count, "profiles": profiles}),
    )?;
    let resp = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}]}});
    respond_value(stdout, &resp).await
}

async fn native_lock(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let (payload, is_error) =
        tokio::task::spawn_blocking(move || cu_core::lock::handle_tool(&args)).await?;
    let text = serde_json::to_string(&payload)?;
    let resp = json!({"jsonrpc":"2.0","id":id,"result":{
        "content":[{"type":"text","text":text}],
        "isError":is_error
    }});
    respond_value(stdout, &resp).await
}

// ¿El puente sirve este tool de forma nativa? (para el lock multi-agente: lo proxeado
// lo bloquea el propio hijo).
fn bridge_serves_native(name: &str, args: &Value) -> bool {
    if matches!(
        name,
        "computer_list_windows"
            | "computer_screenshot"
            | "computer_zoom"
            | "computer_screen_info"
            | "computer_awake"
            | "computer_active_window"
            | "computer_clipboard"
            | "computer_incidents"
            | "computer_lock"
            | "computer_macro_record"
            | "computer_macro_list"
            | "computer_macro_delete"
    ) {
        return true;
    }
    if name == "computer_login" {
        return cu_core::js::flag_truthy(args.get("list_profiles"))
            || (cu_winops::handles(name) && cu_winops::accepts(name, args));
    }
    if CU_EXEC_READY && (name == "computer_batch" || name == "computer_macro_replay") {
        return true;
    }
    (cu_uia::handles(name) && cu_uia::accepts(name, args))
        || (cu_input::handles(name) && cu_input::accepts(name, args))
        || (cu_winops::handles(name) && cu_winops::accepts(name, args))
        || (cu_misc::handles(name) && cu_misc::accepts(name, args))
        || (cu_shell::handles(name) && cu_shell::accepts(name, args))
        || (cu_ocr::handles(name) && cu_ocr::accepts(name, args))
}

// Respuesta de "lock ocupado" (espejo de busyResult del origen).
fn busy_response(msg: &Value, fail: &Value) -> Value {
    let owner = fail.get("owner").cloned().unwrap_or_else(|| json!({}));
    let reason = fail
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("busy")
        .to_string();
    let where_txt = if reason == "desktop_busy" {
        "el desktop".to_string()
    } else {
        format!(
            "la ventana ({})",
            fail.get("blocker_scope")
                .and_then(Value::as_str)
                .or_else(|| owner.get("scope").and_then(Value::as_str))
                .unwrap_or("?")
        )
    };
    let label = owner.get("label").and_then(Value::as_str).unwrap_or("?");
    let pid = owner
        .get("pid")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".to_string());
    let tool = owner.get("tool").and_then(Value::as_str).unwrap_or("?");
    let expires_in_ms = fail.get("expires_in_ms").and_then(Value::as_i64).unwrap_or(0);
    let until = match owner.get("expiresAt") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n
            .as_f64()
            .map(|ms| iso_from_epoch_ms(ms as i64))
            .unwrap_or_else(|| "?".to_string()),
        _ => "?".to_string(),
    };
    let secs = (expires_in_ms as f64 / 1000.0).ceil() as i64;
    let message = format!(
        "Otro agente ({label}, pid {pid}) está usando {where_txt} con '{tool}' hasta {until} (~{secs}s). Reintentá con wait_ms o usá computer_lock {{action:'force', confirm:true}}."
    );
    let payload = json!({
        "ok": false,
        "code": reason,
        "error": reason,
        "owner": fail.get("owner").cloned().unwrap_or(Value::Null),
        "blocker_scope": fail.get("blocker_scope").cloned().unwrap_or(Value::Null),
        "expires_in_ms": expires_in_ms,
        "hint": message,
        "message": message
    });
    json!({"jsonrpc":"2.0","id":msg.get("id").cloned().unwrap_or(Value::Null),"result":{
        "content":[{"type":"text","text":serde_json::to_string(&payload).unwrap_or_default()}],
        "structuredContent":payload,
        "isError":true
    }})
}

// ISO-8601 desde epoch ms (para el expiresAt del lock).
fn iso_from_epoch_ms(ms: i64) -> String {
    cu_core::incidents::iso_from_ms_pub(ms)
}

// Registra un incidente por el resultado de un tool nativo (espejo de recordIncidentSafe).
fn record_native_incident(tool: &str, args: &Value, resp: &Value, duration_ms: i64) {
    cu_core::incidents::record_incident(cu_core::incidents::IncidentRecord {
        tool,
        args,
        error: None,
        result: Some(resp),
        duration_ms: Some(duration_ms as f64),
        version: "",
        session_id: None,
        dir: None,
    });
}

// Incidente por lock ocupado (el origen registra el resultado con duración 0).
fn record_lock_incident(name: &str, args: &Value, fail: &Value) {
    let text = serde_json::to_string(fail).unwrap_or_default();
    let payload = json!({"isError": true, "content": [{"type": "text", "text": text}]});
    cu_core::incidents::record_incident(cu_core::incidents::IncidentRecord {
        tool: name,
        args,
        error: None,
        result: Some(&payload),
        duration_ms: Some(0.0),
        version: "",
        session_id: None,
        dir: None,
    });
}

// Convierte un ToolOut de error en el "result" que consume cu_core::incidents.
fn cluster_error_result(out: &cu_core::out::ToolOut) -> Option<Value> {
    let payload = match out {
        cu_core::out::ToolOut::Json(p, err) | cu_core::out::ToolOut::JsonStructured(p, err) => {
            if !err {
                return None;
            }
            p.clone()
        }
        cu_core::out::ToolOut::Raw(s, err) => {
            if !err {
                return None;
            }
            Value::String(s.clone())
        }
        cu_core::out::ToolOut::Image { .. } | cu_core::out::ToolOut::Link { .. } => return None,
    };
    let text = match &payload {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    Some(json!({"isError": true, "content": [{"type": "text", "text": text}]}))
}

async fn native_cluster(
    name: &str,
    msg: &Value,
    stdout: &mut tokio::io::Stdout,
) -> anyhow::Result<()> {
    // Progress: si el cliente mandó progressToken, emitimos al menos un evento
    // notifications/progress (el origen lo hace durante el polling de wait_stable).
    if let Some(tok) = msg.pointer("/params/_meta/progressToken") {
        let note = json!({"jsonrpc":"2.0","method":"notifications/progress",
            "params": {"progressToken": tok.clone(), "progress": 1, "total": 1}});
        respond_raw(stdout, &note.to_string()).await?;
    }
    let args = msg["params"]["arguments"].clone();
    let args_rec = args.clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let name_owned = name.to_string();
    let out = tokio::task::spawn_blocking(move || {
        if cu_uia::handles(&name_owned) && cu_uia::accepts(&name_owned, &args) {
            cu_uia::handle_call(&name_owned, &args)
        } else if cu_input::handles(&name_owned) && cu_input::accepts(&name_owned, &args) {
            cu_input::handle_call(&name_owned, &args)
        } else if cu_winops::handles(&name_owned) && cu_winops::accepts(&name_owned, &args) {
            cu_winops::handle_call(&name_owned, &args)
        } else if cu_shell::handles(&name_owned) && cu_shell::accepts(&name_owned, &args) {
            cu_shell::handle_call(&name_owned, &args)
        } else if cu_ocr::handles(&name_owned) && cu_ocr::accepts(&name_owned, &args) {
            cu_ocr::handle_call(&name_owned, &args)
        } else {
            cu_misc::handle_call(&name_owned, &args)
        }
    })
    .await?;
    // Incidents: los errores de clusters nativos se registran en el puente (igual que el
    // dispatch del origen); cuando la llamada se proxea, la registra el hijo.
    if let Some(result_v) = cluster_error_result(&out) {
        cu_core::incidents::record_incident(cu_core::incidents::IncidentRecord {
            tool: name,
            args: &args_rec,
            error: None,
            result: Some(&result_v),
            duration_ms: None,
            version: "",
            session_id: None,
            dir: None,
        });
    }
    let resp = match out {
        cu_core::out::ToolOut::Json(p, err) => {
            let text = serde_json::to_string(&p)?;
            json!({"jsonrpc":"2.0","id":id,"result":{
                "content":[{"type":"text","text":text}],
                "isError":err
            }})
        }
        cu_core::out::ToolOut::JsonStructured(p, err) => {
            let text = serde_json::to_string(&p)?;
            json!({"jsonrpc":"2.0","id":id,"result":{
                "content":[{"type":"text","text":text}],
                "structuredContent":p,
                "isError":err
            }})
        }
        cu_core::out::ToolOut::Raw(s, err) => json!({"jsonrpc":"2.0","id":id,"result":{
            "content":[{"type":"text","text":s}],
            "isError":err
        }}),
        cu_core::out::ToolOut::Image { mime, data_b64, text } => json!({"jsonrpc":"2.0","id":id,"result":{"content":[
            {"type":"image","data":data_b64,"mimeType":mime},
            {"type":"text","text":text}
        ]}}),
        cu_core::out::ToolOut::Link { mime, text, uri, name: nm } => json!({"jsonrpc":"2.0","id":id,"result":{"content":[
            {"type":"text","text":text},
            {"type":"resource_link","uri":uri,"name":nm,"mimeType":mime}
        ]}}),
    };
    respond_value(stdout, &resp).await
}

async fn native_macros(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let tool = msg["params"]["name"].as_str().unwrap_or("").to_string();
    let (payload, is_error, structured) = tokio::task::spawn_blocking(move || {
        match tool.as_str() {
            "computer_macro_record" => cu_core::macros::handle_record(&args),
            "computer_macro_list" => cu_core::macros::handle_list(),
            "computer_macro_delete" => cu_core::macros::handle_delete(&args),
            _ => (
                json!({"ok": false, "error": "execution_failed", "message": "unknown macro tool"}),
                true,
                None,
            ),
        }
    })
    .await?;
    let text = serde_json::to_string(&payload)?;
    let mut result = json!({"content": [{"type": "text", "text": text}]});
    match structured {
        Some(sc) => {
            result["structuredContent"] = sc;
        }
        None => {
            result["isError"] = json!(is_error);
        }
    }
    let resp = json!({"jsonrpc":"2.0","id":id,"result":result});
    respond_value(stdout, &resp).await
}

// --- Dispatcher interno (computer_batch / computer_macro_replay) ---------------
//
// Las sub-llamadas se resuelven primero contra los clusters nativos del puente; si el tool
// no es nativo, se reenvía a un SEGUNDO hijo Node sincrónico (mismo código del origen) que
// se inicializa una vez y queda vivo. Es sincrónico porque cu-exec expone un Executor sync.

struct SyncChild {
    _child: std::process::Child,
    stdin: std::process::ChildStdin,
    // Lector dedicado: las líneas del hijo llegan por un canal con plazo. Una lectura
    // bloqueada sin timeout puede colgar el puente entero.
    rx: std::sync::mpsc::Receiver<String>,
    next_id: i64,
}

impl Drop for SyncChild {
    fn drop(&mut self) {
        let _ = self._child.kill();
    }
}

impl SyncChild {
    fn spawn() -> Result<Self, String> {
        let mut cmd = std::process::Command::new(ORIGIN_NODE);
        cmd.arg(ORIGIN_SCRIPT)
            .current_dir(ORIGIN_CWD)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("hijo sin stdin")?;
        let stdout = child.stdout.take().ok_or("hijo sin stdout")?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.clone()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut me = SyncChild {
            _child: child,
            stdin,
            rx,
            next_id: 1000,
        };
        me.init()?;
        Ok(me)
    }

    fn write_line(&mut self, line: &str) -> Result<(), String> {
        use std::io::Write;
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| e.to_string())?;
        self.stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())
    }

    fn read_message(&mut self) -> Result<Value, String> {
        loop {
            let line = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .map_err(|e| e.to_string())?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                return Ok(v);
            }
        }
    }

    fn init(&mut self) -> Result<(), String> {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2024-11-05",
            "capabilities":{},
            "clientInfo":{"name":"cu2-bridge-exec","version":"0.1.0"}
        }});
        self.write_line(&serde_json::to_string(&init).map_err(|e| e.to_string())?)?;
        self.write_line("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}")?;
        for _ in 0..50 {
            let m = self.read_message()?;
            if m.get("id").and_then(Value::as_i64) == Some(1) {
                return Ok(());
            }
        }
        Err("el hijo no respondió al initialize".to_string())
    }

    fn call(&mut self, tool: &str, args: &Value) -> Result<(Value, bool), String> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":tool,"arguments":args}});
        self.write_line(&serde_json::to_string(&req).map_err(|e| e.to_string())?)?;
        for _ in 0..500 {
            let m = self.read_message()?;
            if m.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(err) = m.get("error") {
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error interno");
                return Err(msg.to_string());
            }
            let result = m.get("result").cloned().unwrap_or(Value::Null);
            let is_error = result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let text = result
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or("");
            let payload: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            return Ok((payload, is_error));
        }
        Err("timeout en sub-llamada".to_string())
    }
}

fn exec_child() -> &'static Arc<std::sync::Mutex<Option<SyncChild>>> {
    static CHILD: OnceLock<Arc<std::sync::Mutex<Option<SyncChild>>>> = OnceLock::new();
    CHILD.get_or_init(|| Arc::new(std::sync::Mutex::new(None)))
}

// Convierte un ToolOut de cluster en (payload, isError) para el Executor.
fn toolout_payload(out: &cu_core::out::ToolOut) -> (Value, bool) {
    match out {
        cu_core::out::ToolOut::Json(p, e) => (p.clone(), *e),
        cu_core::out::ToolOut::JsonStructured(p, e) => (p.clone(), *e),
        cu_core::out::ToolOut::Raw(s, e) => (Value::String(s.clone()), *e),
        cu_core::out::ToolOut::Image { text, .. } => (Value::String(text.clone()), false),
        cu_core::out::ToolOut::Link { text, .. } => (Value::String(text.clone()), false),
    }
}

// Sub-llamada servida por un cluster nativo (mismo orden que el dispatch del puente).
fn native_cluster_call(tool: &str, args: &Value) -> Option<(Value, bool)> {
    let out = if cu_uia::handles(tool) && cu_uia::accepts(tool, args) {
        cu_uia::handle_call(tool, args)
    } else if cu_input::handles(tool) && cu_input::accepts(tool, args) {
        cu_input::handle_call(tool, args)
    } else if cu_winops::handles(tool) && cu_winops::accepts(tool, args) {
        cu_winops::handle_call(tool, args)
    } else if cu_misc::handles(tool) && cu_misc::accepts(tool, args) {
        cu_misc::handle_call(tool, args)
    } else if cu_shell::handles(tool) && cu_shell::accepts(tool, args) {
        cu_shell::handle_call(tool, args)
    } else if cu_ocr::handles(tool) && cu_ocr::accepts(tool, args) {
        cu_ocr::handle_call(tool, args)
    } else {
        return None;
    };
    Some(toolout_payload(&out))
}

// Sub-llamada de macros servida por cu-core (mismo handler que el puente).
fn native_macro_call(tool: &str, args: &Value) -> Option<(Value, bool)> {
    let (payload, is_err, _structured) = match tool {
        "computer_macro_record" => cu_core::macros::handle_record(args),
        "computer_macro_list" => cu_core::macros::handle_list(),
        "computer_macro_delete" => cu_core::macros::handle_delete(args),
        _ => return None,
    };
    Some((payload, is_err))
}

struct BridgeExec {
    child: Arc<std::sync::Mutex<Option<SyncChild>>>,
    // Progress del replay/batch: el Executor acumula las notificaciones y el handler
    // las emite antes de la respuesta final (el origen emite una por paso).
    progress: Arc<std::sync::Mutex<Vec<Value>>>,
    token: Option<Value>,
    steps: Arc<AtomicU64>,
}

impl cu_exec::Executor for BridgeExec {
    fn call(&self, tool: &str, args: &Value) -> Result<(Value, bool), String> {
        let res = if let Some(res) = native_cluster_call(tool, args) {
            Ok(res)
        } else if let Some(res) = native_macro_call(tool, args) {
            Ok(res)
        } else {
            let mut guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some(SyncChild::spawn()?);
            }
            let child = guard.as_mut().ok_or("hijo no disponible")?;
            child.call(tool, args)
        };
        if let Some(tok) = self.token.as_ref() {
            let step = self.steps.fetch_add(1, Ordering::SeqCst) + 1;
            let note = json!({"jsonrpc":"2.0","method":"notifications/progress",
                "params":{"progressToken":tok.clone(),"progress":step,
                    "message":format!("{tool} (paso {step})")}});
            if let Ok(mut g) = self.progress.lock() {
                g.push(note);
            }
        }
        res
    }

    // Elementos UIA para el healing del replay: los pide al hijo Node (misma
    // implementación que el origen usa para collectElements).
    fn collect_elements(&self, target: &Value) -> Vec<Value> {
        let mut args = serde_json::Map::new();
        if let Some(h) = target.get("hwnd") {
            args.insert("hwnd".into(), h.clone());
        }
        if let Some(t) = target.get("target_window") {
            args.insert("target_window".into(), t.clone());
        }
        args.insert("max_results".into(), json!(300));
        args.insert("interactive_only".into(), json!(true));
        let mut guard = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            match SyncChild::spawn() {
                Ok(c) => *guard = Some(c),
                Err(_) => return Vec::new(),
            }
        }
        let Some(child) = guard.as_mut() else {
            return Vec::new();
        };
        match child.call("computer_inspect_ui", &Value::Object(args)) {
            Ok((payload, _)) => payload
                .get("elements")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    fn active_window(&self) -> Option<Value> {
        Some(cu_native::system::active_window())
    }
}

async fn native_exec(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let args = msg["params"]["arguments"].clone();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
    let token = msg.pointer("/params/_meta/progressToken").cloned();
    let progress: Arc<std::sync::Mutex<Vec<Value>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let exec = BridgeExec {
        child: exec_child().clone(),
        progress: progress.clone(),
        token,
        steps: Arc::new(AtomicU64::new(0)),
    };
    let out = tokio::task::spawn_blocking(move || cu_exec::handle_call(&name, &args, &exec)).await?;
    // Las notificaciones de progreso se emiten antes de la respuesta final.
    if let Ok(notes) = progress.lock() {
        for note in notes.iter() {
            respond_raw(stdout, &note.to_string()).await?;
        }
    }
    let resp = match out {
        cu_core::out::ToolOut::Json(p, err) => {
            let text = serde_json::to_string(&p)?;
            json!({"jsonrpc":"2.0","id":id,"result":{
                "content":[{"type":"text","text":text}],
                "isError":err
            }})
        }
        cu_core::out::ToolOut::JsonStructured(p, err) => {
            let text = serde_json::to_string(&p)?;
            json!({"jsonrpc":"2.0","id":id,"result":{
                "content":[{"type":"text","text":text}],
                "structuredContent":p,
                "isError":err
            }})
        }
        cu_core::out::ToolOut::Raw(s, err) => json!({"jsonrpc":"2.0","id":id,"result":{
            "content":[{"type":"text","text":s}],
            "isError":err
        }}),
        cu_core::out::ToolOut::Image { mime, data_b64, text } => json!({"jsonrpc":"2.0","id":id,"result":{"content":[
            {"type":"image","data":data_b64,"mimeType":mime},
            {"type":"text","text":text}
        ]}}),
        cu_core::out::ToolOut::Link { mime, text, uri, name: nm } => json!({"jsonrpc":"2.0","id":id,"result":{"content":[
            {"type":"text","text":text},
            {"type":"resource_link","uri":uri,"name":nm,"mimeType":mime}
        ]}}),
    };
    respond_value(stdout, &resp).await
}

async fn bootstrap_child(child_in: &mut Option<ChildStdin>) -> anyhow::Result<()> {
    // Inicializa el child Node con id interno negativo: sus respuestas no se reenvian al cliente.
    let Some(ci) = child_in.as_mut() else {
        return Ok(());
    };
    let init = json!({
        "jsonrpc": "2.0",
        "id": -1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "cu2-bridge", "version": "0.1.0" }
        }
    });
    ci.write_all(serde_json::to_string(&init)?.as_bytes()).await?;
    ci.write_all(b"\n").await?;
    ci.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n")
        .await?;
    ci.flush().await?;
    Ok(())
}

fn spawn_origin() -> anyhow::Result<(Option<Child>, Option<ChildStdin>, Option<Lines<BufReader<ChildStdout>>>)> {
    let mut cmd = Command::new(ORIGIN_NODE);
    cmd.arg(ORIGIN_SCRIPT)
        .current_dir(ORIGIN_CWD)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().map(|s| BufReader::new(s).lines());
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                eprintln!("[origin] {l}");
            }
        });
    }
    Ok((Some(child), stdin, stdout))
}

async fn forward(child_in: &mut Option<ChildStdin>, line: &str) -> anyhow::Result<()> {
    if let Some(ci) = child_in.as_mut() {
        ci.write_all(line.as_bytes()).await?;
        ci.write_all(b"\n").await?;
        ci.flush().await?;
    }
    Ok(())
}

async fn respond_value(stdout: &mut tokio::io::Stdout, v: &Value) -> anyhow::Result<()> {
    // Guarda la última respuesta (con id) para el registro de incidentes del origen.
    if v.get("id").is_some() {
        if let Ok(mut slot) = LAST_RESULT.lock() {
            *slot = Some(v.clone());
        }
    }
    let s = serde_json::to_string(v)?;
    respond_raw(stdout, &s).await
}

async fn respond_raw(stdout: &mut tokio::io::Stdout, s: &str) -> anyhow::Result<()> {
    stdout.write_all(s.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

async fn native_list_windows(msg: &Value, stdout: &mut tokio::io::Stdout) -> anyhow::Result<()> {
    let filter = msg["params"]["arguments"]["filter"]
        .as_str()
        .map(|s| s.to_string());
    let windows = tokio::task::spawn_blocking(move || cu_native::list_windows(filter.as_deref()))
        .await
        .map_err(|e| anyhow::anyhow!("join: {e}"))?;
    let capped: Vec<_> = windows.into_iter().take(20).collect();
    let text = serde_json::to_string(&capped)?;
    let resp = json!({
        "jsonrpc": "2.0",
        "id": msg.get("id").cloned().unwrap_or(Value::Null),
        "result": {
            "content": [ { "type": "text", "text": text } ],
            "structuredContent": { "windows": capped }
        }
    });
    respond_value(stdout, &resp).await
}
