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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

static SHOT_COUNTER: AtomicU64 = AtomicU64::new(0);

const ORIGIN_NODE: &str = "node";
const ORIGIN_SCRIPT: &str = "G:/Proyectos/MCP/computer-use2/src/index.js";
const ORIGIN_CWD: &str = "G:/Proyectos/MCP/computer-use2";

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
                        // Grabación de macros: registra TODO tool call (salvo los macro_* y los
                        // bootstrap internos de id negativo) con args saneados, igual que el
                        // dispatch del origen. El replay lo ejecuta el hijo sobre el mismo archivo.
                        if !name.is_empty()
                            && !name.starts_with("computer_macro_")
                            && msg
                                .get("id")
                                .and_then(Value::as_i64)
                                .map(|id| id >= 0)
                                .unwrap_or(true)
                            && cu_core::macros::is_recording()
                        {
                            let args_v = msg
                                .get("params")
                                .and_then(|p| p.get("arguments"))
                                .cloned()
                                .unwrap_or_else(|| json!({}));
                            cu_core::macros::note_step(name, &args_v);
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
            let epoch = cu_core::epoch::bump();
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
            let epoch = cu_core::epoch::bump();
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
