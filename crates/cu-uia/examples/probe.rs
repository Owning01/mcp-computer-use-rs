use cu_core::out::ToolOut;
use cu_uia;
use serde_json::json;

fn main() {
    let out = cu_uia::handle_call(
        "computer_background_action",
        &json!({"action":"invoke","hwnd":1}),
    );
    match out {
        ToolOut::Json(v, err) => println!("bg err={} {}", err, v.to_string()),
        _ => println!("other"),
    }
}
