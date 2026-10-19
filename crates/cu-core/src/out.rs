// ToolOut — forma de respuesta de los clusters nativos del puente.
// Json(Value, bool): texto = JSON.stringify(payload); bool = isError.
// Raw(String, bool): texto crudo (sin JSON.stringify; p.ej. clipboard get).
// Image: content [image, text]. Link: content [text, resource_link].
use serde_json::Value;

pub enum ToolOut {
    Json(Value, bool),
    // structuredContent = payload (emula el auto-derive del SDK para tools con outputSchema).
    JsonStructured(Value, bool),
    Raw(String, bool),
    Image {
        mime: String,
        data_b64: String,
        text: String,
    },
    Link {
        mime: String,
        text: String,
        uri: String,
        name: String,
    },
}
