use cu_shell::{handle_call, handles};
use serde_json::json;

fn main() {
    println!("handles indicator: {}", handles("computer_indicator"));
    let show = handle_call(
        "computer_indicator",
        &json!({"action":"show","message":"Probe","duration_ms":500}),
    );
    match show {
        cu_core::out::ToolOut::Json(v, e) => println!("show: {} (isError={})", v, e),
        _ => println!("show: other"),
    }
    std::thread::sleep(std::time::Duration::from_millis(700));
    let hide = handle_call("computer_indicator", &json!({"action":"hide"}));
    match hide {
        cu_core::out::ToolOut::Json(v, e) => println!("hide: {} (isError={})", v, e),
        _ => println!("hide: other"),
    }
}
