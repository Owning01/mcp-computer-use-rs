# PROJECT_MAP — computer-use2-rs

## Qué es
MCP de control de escritorio Windows (68 tools; 12 en perfil agent). Reemplazo Rust del origen
`G:\Proyectos\MCP\computer-use2` (Node.js ESM + PowerShell 5.1 + C# net9 + Python). Programa repo-migrate (F0→F8).

## Estado
- F0–F5 COMPLETOS. F6 en curso: **puente JSON-RPC crudo** con 8 de 68 tools nativos; el resto se proxea al hijo Node (cero regresión).
- Tools nativos: `computer_list_windows`, `computer_screenshot`, `computer_zoom`, `computer_screen_info`, `computer_awake`,
  `computer_active_window`, `computer_clipboard`, `computer_incidents`.
- Sin cutover: `opencode.json` sigue apuntando al origen Node. El binario Rust se valida con el harness de paridad (juez + smoke congelado).
- Divergencias documentadas y aceptadas: WGC real pendiente (`wgc_unavailable` controlado); `cursor`/`annotate_marks` TODO(port); PaddleOCR sidecar.

## Estructura
- `crates/cu-mcp` — binario `computer-use2-rs.exe`: puente JSON-RPC crudo; intercepta los tools nativos y proxea el resto al hijo Node (`node src/index.js`).
- `crates/cu-native` — primitivas Win32 (windows-rs 0.62.2): list_windows (DPI PMv2), capture/screenshot (DPI unaware), screen, system (awake/clipboard/active_window).
- `crates/cu-core` — lógica portable sin Win32: incidents (journal JSONL compartido con el origen).
- `migration/` — CHARTER, RULEBOOK (firmados), API-MAPPING, ORACLE, `parity/` (juez, escenarios, smoke congelado, mutantes, bench).
- `migration/parity/oracle/contract-smoke.frozen.mjs` — oráculo congelado (68 tools, fases annotation/outputSchema/structuredContent/resource_link/progress/behavior/wgc/tasks/lock/incidents).

## Comandos (desde la raíz del repo)
- Build: `cargo build --release` → `target/release/computer-use2-rs.exe`.
- Smoke congelado: `$env:PARITY_SERVER_CMD='["<bin>"]'; node migration\parity\oracle\contract-smoke.frozen.mjs`.
- Juez de paridad: `node migration\parity\judge.mjs migration\parity\scenarios\cu2-basic.json --b-cmd '["<bin>"]'`.
- Bench: `node migration\parity\bench.mjs '["<bin>"]' --call '{"name":"computer_screen_info","arguments":{}}'`.

## Versiones
- Rust edition 2024 (`rust-toolchain.toml`), windows-rs 0.62.2, serde_json con `preserve_order`, tokio 1.53.

## Trampas
- DPI por contexto (quirk del origen, replicado por-tool): `list_windows`/`screen_info`/`active_window` = PER_MONITOR_AWARE_V2 (físico); `screenshot` = UNAWARE (virtualizado).
- Órdenes de claves fijados por el origen (`ConvertTo-Json` de PS 5.1, `JSON.stringify`); el juez compara byte a byte → los payloads nativos se construyen en ese orden exacto.
- Spawns no-interactivos: `stdin(Stdio::null())` (regla del rulebook; evita cuelgues).
- El origen sigue siendo el server activo de OpenCode hasta el cutover; no tocar su config antes.
- Cutover de cu2 pendiente: 1 línea de `opencode.json` (rollback = restaurar la línea del origen).
