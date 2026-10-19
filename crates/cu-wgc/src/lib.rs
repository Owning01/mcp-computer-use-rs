// cu-wgc — captura Windows.Graphics.Capture (WinRT) para el puente. Reglas: migration/RULEBOOK.md.
// API acordada con cu-native::capture (la integra el puente en F6):
//   capture(hwnd, timeout_ms) -> Result<Frame, String>; Frame { bgra (top-down, BGRA, fila * width * 4), width, height }
// Errores con prefijo "wgc_" (p.ej. wgc_unavailable, wgc_timeout) como el helper C# del origen.
// Stub: sin implementar todavia (lo completa la fase F6).
pub struct Frame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn capture(_hwnd: i64, _timeout_ms: u64) -> Result<Frame, String> {
    Err("wgc_unavailable".to_string())
}
