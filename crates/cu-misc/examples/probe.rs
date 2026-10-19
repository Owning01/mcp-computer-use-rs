use cu_core::out::ToolOut;
use cu_misc;
use serde_json::json;

fn show(name: &str, r: ToolOut) {
    match r {
        ToolOut::Json(v, e) => println!("{}: err={} {}", name, e, v),
        ToolOut::Raw(s, e) => println!("{}: err={} {}", name, e, s),
        _ => println!("{}: other", name),
    }
}

fn main() {
    show("process_tree", cu_misc::handle_call("computer_process_tree", &json!({"max_depth": 2})));
    show("check_health", cu_misc::handle_call("computer_check_health", &json!({})));
    show("alert_user", cu_misc::handle_call("computer_alert_user", &json!({"message": "probe test", "duration_ms": 2000, "sound": false})));
    show("calibrate_dpi", cu_misc::handle_call("computer_calibrate_dpi", &json!({})));
    show("audio status", cu_misc::handle_call("computer_audio_action", &json!({"action": "status"})));
    show("detect_block", cu_misc::handle_call("computer_detect_block", &json!({})));
    show("pass_barrier", cu_misc::handle_call("computer_pass_barrier", &json!({"strategy": "ask_user", "max_attempts": 1, "timeout_ms": 2000})));
    show("dismiss_overlay", cu_misc::handle_call("computer_dismiss_overlay", &json!({"max_attempts": 1})));
    show("shell", cu_misc::handle_call("computer_shell", &json!({"command": "echo hello"})));
    show("shell_exec", cu_misc::handle_call("shell_exec", &json!({"command": "echo hello123"})));
    show("shell_bg", cu_misc::handle_call("shell_background", &json!({"command": "echo bg"})));
    show("wait_download", cu_misc::handle_call("computer_wait_download", &json!({"timeout_seconds": 1})));

    // audio mute toggle restore: read current, mute then unmute
    if let ToolOut::Json(v, _) = cu_misc::handle_call("computer_audio_action", &json!({"action": "status"})) {
        let was_muted = v.get("muted").and_then(|x| x.as_bool()).unwrap_or(false);
        let vol = v.get("volume").and_then(|x| x.as_i64()).unwrap_or(50);
        println!("before: muted={} vol={}", was_muted, vol);
        let _ = cu_misc::handle_call("computer_audio_action", &json!({"action": "mute"}));
        std::thread::sleep(std::time::Duration::from_millis(300));
        show("after mute", cu_misc::handle_call("computer_audio_action", &json!({"action": "status"})));
        // restore
        if !was_muted {
            let _ = cu_misc::handle_call("computer_audio_action", &json!({"action": "unmute"}));
        }
        let _ = cu_misc::handle_call("computer_audio_action", &json!({"action": "set_volume", "volume": vol}));
        println!("restored");
    }
}