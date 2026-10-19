use cu_core::out::ToolOut;
use serde_json::json;

fn main() {
    let args = json!({
        "x": 0,
        "y": 0,
        "w": 1200,
        "h": 800,
        "language": "en",
        "device": "auto",
        "max_results": 20
    });
    match cu_ocr::handle_call("computer_ocr", &args) {
        ToolOut::Json(payload, _) | ToolOut::JsonStructured(payload, _) => {
            let items = payload
                .get("items")
                .and_then(serde_json::Value::as_array)
                .map(|items| items.iter().take(3).cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            println!(
                "{}",
                json!({
                    "backend": payload.get("backend"),
                    "device": payload.get("device"),
                    "items": items,
                    "error": payload.get("error")
                })
            );
        }
        _ => println!("respuesta OCR inesperada"),
    }
}
