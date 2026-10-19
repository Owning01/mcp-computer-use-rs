# PROJECT_MAP — computer-use2-rs

## Qué es
MCP de control de escritorio Windows (68 tools; 12 en perfil agent). Reemplazo Rust del origen
`G:\Proyectos\MCP\computer-use2` (Node.js ESM + PowerShell 5.1 + C# net9 + Python). Programa repo-migrate (F0→F8).

## Estado
- F0–F6 CERRADOS. **Cutover F8 hecho**: `opencode.json` arranca el binario Rust en la entrada `computer-use` (backup en `opencode.json.bak-f6`). El origen SIGUE en disco: el puente proxea `computer_upload_file`, las ramas que `accepts` rechaza y la extensión tasks. 67 de 68 tools servidos nativamente.
- Tools nativos — base (13): `computer_list_windows`, `computer_screenshot` (+WGC real), `computer_zoom`, `computer_screen_info`, `computer_awake`, `computer_active_window`, `computer_clipboard`, `computer_incidents`, `computer_login` (completo: list_profiles + launch), `computer_lock`, `computer_macro_record`, `computer_macro_list`, `computer_macro_delete`.
- Clusters (54): UIA (19) — `inspect_ui`, `snapshot`, `state_fingerprint`, `verify`, `wait_stable` (sync), `find_element`, `map_ui`, `find_template`, `ui_action`, `read_selection`, `observe`, `system_tray`, `context_menu`, `toast_action`, `background_action`, `click_text`, `fill_form`, `scroll_to_text`, `act` · input (14) — `hotkey`, `key`, `move`, `copy`, `paste`, `bulk_input`, `drag_scroll`, `click`, `double_click`, `human_click`, `drag`, `scroll`, `type`, `handle_file_dialog` · ventanas (5) — `focus_window`, `window_action`, `wait_for`, `monitor_action`, `wait` · varios (12) — `process_tree`, `check_health`, `calibrate_dpi`, `alert_user`, `audio_action`, `detect_block`, `pass_barrier`, `dismiss_overlay`, `computer_shell`, `shell_exec`, `shell_background`, `wait_download` · shell (1) — `indicator` · ocr (1) — `computer_ocr` · exec (2) — `computer_batch`, `computer_macro_replay`.
- Capa de seguridad del puente: bloqueo de gestores de contraseñas (`cu-core::blocked`), lock multi-agente para tools nativos (`cu-core::plan` + `cu-core::lock`, respuesta `busy` del origen), epoch único compartido con el hijo, incidentes de todo tool nativo que falla, y timeout de lectura del dispatcher interno.
- Progress notifications del puente: native_cluster emite `notifications/progress` si el cliente manda `_meta.progressToken` (el replay nativo emite uno por paso).
- Pendiente (1): `computer_upload_file` — divergencia F7: su Tier 1 es inyección CDP (requiere cliente DevTools).
otifications/progress si el cliente manda _meta.progressToken (el replay nativo emite uno por paso).
- Pendiente (1): computer_upload_file - divergencia F7: su Tier 1 es inyeccion CDP (requiere cliente DevTools).
- Routing especial: mientras el hijo Node graba una macro (`computer_macro_record` start..stop, espejado en el puente), el puente NO intercepta clusters: la llamada va al hijo para quedar registrada.
- Sin cutover: `opencode.json` sigue apuntando al origen Node. El binario Rust se valida con el harness de paridad (juez + smoke congelado).
- Divergencias documentadas y aceptadas: WGC real pendiente (`wgc_unavailable` controlado); `cursor`/`annotate_marks` TODO(port); PaddleOCR sidecar.

## Estructura
- `crates/cu-mcp` — binario `computer-use2-rs.exe`: puente JSON-RPC crudo; intercepta los tools nativos y proxea el resto al hijo Node (`node src/index.js`).
- `crates/cu-native` — primitivas Win32 (windows-rs 0.62.2): list_windows (DPI PMv2), capture/screenshot (DPI unaware), screen, system (awake/clipboard/active_window).
- `crates/cu-core` — lógica portable sin Win32: incidents (journal JSONL compartido con el origen), lock, profiles, epoch, ToolOut.
- `crates/cu-uia`, `crates/cu-input`, `crates/cu-winops`, `crates/cu-misc`, `crates/cu-shell`, `crates/cu-wgc`, `crates/cu-ocr`, `crates/cu-exec` — clusters F6 (contrato `handles`/`accepts`/`handle_call` → `ToolOut`; stub vacío = todo proxeado). `cu-exec` usa un `Executor` inyectado por el puente.
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
- Tooling (PowerShell): no pipear `cargo ... | Select-Object -First N` — mata el build a mitad; capturar la salida completa en variable y después filtrar.
- El origen sigue siendo el server activo de OpenCode hasta el cutover; no tocar su config antes.
- Cutover de cu2 pendiente: 1 línea de `opencode.json` (rollback = restaurar la línea del origen).
