use cu_core::out::ToolOut;
use cu_uia;
use serde_json::json;

fn main() {
    let out = cu_uia::handle_call("computer_system_tray", &json!({"action":"list"}));
    match out {
        ToolOut::Json(v, err) => println!("tray err={} count={}", err, v.get("count").unwrap_or(&json!(0))),
        _ => println!("other"),
    }
    let out = cu_uia::handle_call("computer_toast_action", &json!({"action":"click_button","button_text":"OK"}));
    match out {
        ToolOut::Json(v, err) => println!("toast err={} {}", err, v.to_string().chars().take(120).collect::<String>()),
        _ => println!("other"),
    }
}