// cu-misc — tools varios (process_tree, check_health, calibrate_dpi, alert_user, audio).
// Contrato del puente: handles(name) + handle_call(name, args) -> cu_core::out::ToolOut.
mod alert_user;
mod audio;
mod calibrate_dpi;
mod check_health;
mod detect_block;
mod dismiss_overlay;
mod pass_barrier;
mod process_tree;
mod shell;
mod wait_download;

use cu_core::out::ToolOut;
use serde_json::{json, Value};

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "computer_process_tree"
            | "computer_check_health"
            | "computer_calibrate_dpi"
            | "computer_alert_user"
            | "computer_audio_action"
            | "computer_pass_barrier"
            | "computer_detect_block"
            | "computer_dismiss_overlay"
            | "computer_shell"
            | "shell_exec"
            | "shell_background"
            | "computer_wait_download"
    )
}

// accepts: subconjunto de args servible nativo (default true; el puente consulta esto).
pub fn accepts(_name: &str, args: &Value) -> bool {
    let isolated = args.get("isolated_desktop").and_then(|v| v.as_bool()).unwrap_or(false);
    if isolated {
        // runInDesktopSandbox no disponible nativo
        return false;
    }
    true
}

pub fn handle_call(name: &str, args: &Value) -> ToolOut {
    match name {
        "computer_process_tree" => {
            let v = process_tree::process_tree(args);
            ToolOut::Json(v, false)
        }
        "computer_check_health" => {
            let v = check_health::check_health(args);
            ToolOut::Json(v, false)
        }
        "computer_calibrate_dpi" => {
            let v = calibrate_dpi::calibrate_dpi(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            ToolOut::Json(v, err)
        }
        "computer_alert_user" => {
            let v = alert_user::alert_user(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            ToolOut::Json(v, err)
        }
        "computer_audio_action" => {
            let v = audio::audio_action(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            ToolOut::Json(v, err)
        }
        "computer_pass_barrier" => {
            let v = pass_barrier::pass_barrier(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            ToolOut::Json(v, err)
        }
        "computer_detect_block" => {
            let v = detect_block::detect_block(args);
            ToolOut::Json(v, false)
        }
        "computer_dismiss_overlay" => {
            let v = dismiss_overlay::dismiss_overlay(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            ToolOut::Json(v, err)
        }
        "computer_shell" => {
            let v = shell::shell(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            if let Some(out) = v.get("output").and_then(|x| x.as_str()) {
                ToolOut::Raw(out.to_string(), err)
            } else {
                ToolOut::Json(v, err)
            }
        }
        "shell_exec" => {
            let v = shell::shell_exec(args);
            let text = v.get("_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let is_err = v.get("_is_error").and_then(|x| x.as_bool()).unwrap_or(v.get("ok").and_then(|x| x.as_bool()) != Some(true));
            ToolOut::Raw(text, is_err)
        }
        "shell_background" => {
            let v = shell::shell_background(args);
            let text = v.get("_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
            ToolOut::Raw(text, false)
        }
        "computer_wait_download" => {
            let v = wait_download::wait_download(args);
            let err = v.get("ok").and_then(|x| x.as_bool()) != Some(true);
            let epoch = cu_core::epoch::bump();
            let mut v = v;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("epoch".to_string(), json!(epoch));
            }
            ToolOut::Json(v, err)
        }
        _ => ToolOut::Json(json!({"ok": false, "error": "not_implemented"}), true),
    }
}