# API-MAPPING — computer-use2-rs (F3)

Mapea el código origen a los crates destino. Se actualiza en cada batch (F6) y es parte del gate.
Borrador v1 — 2026-10-18. Correcciones de frontera se deciden en F4 (corte vertical).

## 1. Capas origen → crates destino

| Origen | Destino | Nota |
| :--- | :--- | :--- |
| `src/index.js` (6.4k líneas: registro de tools, router, tools inline, tasks, guardrails) | `cu-mcp` (bin) + `cu-core` | Se parte: protocolo/registro → `cu-mcp`; lógica → `cu-core`. |
| `src/handlers/{lock,incidents,macros,tasks,agent_core,verify,snapshot,background,barrier}.js` | `cu-core` | Lógica portable; llaman a `cu-native` por interfaz tipada. |
| `src/utils/error_recovery.js` | `cu-core` | |
| `src/utils/{ps_daemon,ps_runner}.js` | **ELIMINADOS** | Reemplazados por llamadas nativas directas. |
| `src/handlers/{capture,calibrate_dpi,audio_action,awake,indicator,system_tray,toast_action,context_menu,drag_scroll}.js` + `native/**` (PS 1.4k + C# 1.6k) | `cu-native` | Todo reescrito con `windows-rs`. |
| `src/handlers/sandbox.js` + `native/DesktopBridge.exe` | `cu-native` | `CreateDesktop`/`SetThreadDesktop` in-process. |
| `src/utils/browser_cdp.js` + `browser_profiles.js` | `cu-cdp` | CDP por `tokio-tungstenite` + `reqwest`. |
| `src/ocr_worker.py` | `cu-ocr` | `Windows.Media.Ocr`; PaddleOCR sidecar pendiente de spike (charter §3). |
| `tests/` (27 archivos) | Referencia para tests Rust + oráculo | Ver `migration/ORACLE.md` (categorización). |

## 2. Los 68 tools → crate

Todos expuestos por `cu-mcp`; la columna indica dónde vive la implementación.

| # | Tool | Crate | Fuente principal (origen) |
| :-- | :--- | :--- | :--- |
| 1 | computer_screenshot | native | capture.js |
| 2 | computer_act | core | agent_core.js (orquesta primitivas nativas) |
| 3 | computer_observe | core+native | agent_core.js (UIA + formato compacto) |
| 4 | computer_shell | core+native | agent_core.js handleShell |
| 5 | computer_wait | core | agent_core.js handleWait |
| 6 | computer_zoom | native | capture.js (región escalada) |
| 7 | computer_screen_info | native | index.js |
| 8 | computer_hotkey | native | input (SendInput) |
| 9 | computer_click_text | native+core | UIA find + input |
| 10 | computer_map_ui | cdp | browser_cdp.js (refs `c1`,`c2` DOM) |
| 11 | computer_click | native | input |
| 12 | computer_double_click | native | input |
| 13 | computer_human_click | native | input (Bézier) |
| 14 | computer_copy | native | clipboard |
| 15 | computer_paste | native | clipboard |
| 16 | computer_fill_form | core | index.js (multi-campo) |
| 17 | computer_detect_block | core+native | barrier.js |
| 18 | computer_background_action | native | background.js (UIA sin foco) |
| 19 | computer_pass_barrier | core | barrier.js |
| 20 | computer_move | native | input |
| 21 | computer_drag | native | input |
| 22 | computer_key | native | input |
| 23 | computer_type | native | input (cadencia humanizada) |
| 24 | computer_scroll_to_text | native+core | drag_scroll.js |
| 25 | computer_handle_file_dialog | native | index.js (hook `#32770`) |
| 26 | computer_window_action | native | index.js |
| 27 | computer_upload_file | native+cdp | 3 tiers (CDP / diálogo / OLE) |
| 28 | computer_alert_user | native | toast/indicator |
| 29 | computer_scroll | native | input |
| 30 | computer_batch | core | index.js |
| 31 | computer_clipboard | native | clipboard |
| 32 | computer_focus_window | native | index.js |
| 33 | computer_active_window | native | index.js |
| 34 | computer_wait_for | core | index.js |
| 35 | computer_login | cdp+core | browser_profiles + CDP |
| 36 | computer_find_element | native | UIA |
| 37 | computer_inspect_ui | native | UIA |
| 38 | computer_ui_action | native | UIA invoke |
| 39 | computer_snapshot | native+core | snapshot.js |
| 40 | computer_state_fingerprint | core+native | sha1 sobre estado |
| 41 | computer_wait_stable | core | index.js (estabilización) |
| 42 | computer_verify | core+native | verify.js |
| 43 | computer_macro_record | core | macros.js |
| 44 | computer_macro_list | core | macros.js |
| 45 | computer_macro_delete | core | macros.js |
| 46 | computer_macro_replay | core | macros.js |
| 47 | computer_process_tree | native | index.js (Toolhelp32) |
| 48 | computer_ocr | ocr | index.js + ocr_worker.py |
| 49 | computer_list_windows | native | index.js (EnumWindows) |
| 50 | shell_exec | core+native | index.js |
| 51 | shell_background | core+native | index.js |
| 52 | computer_wait_download | core | index.js |
| 53 | computer_find_template | native | template match |
| 54 | computer_monitor_action | native | index.js |
| 55 | computer_read_selection | native | UIA text pattern |
| 56 | computer_check_health | core | index.js |
| 57 | computer_bulk_input | native | input |
| 58 | computer_system_tray | native | system_tray.js |
| 59 | computer_toast_action | native | toast_action.js |
| 60 | computer_context_menu | native | context_menu.js |
| 61 | computer_drag_scroll | native | drag_scroll.js |
| 62 | computer_calibrate_dpi | native | calibrate_dpi.js |
| 63 | computer_audio_action | native | audio_action.js (CoreAudio) |
| 64 | computer_lock | core | lock.js (CAS/leases por archivo) |
| 65 | computer_incidents | core | incidents.js |
| 66 | computer_awake | native | awake.js (SetThreadExecutionState) |
| 67 | computer_indicator | native | indicator.js (HUD nativo) |
| 68 | computer_dismiss_overlay | core | index.js |

## 3. Interfaz `cu-core` ↔ `cu-native`

- `cu-native` expone funciones tipadas puras (`capture_window(hwnd, opts) -> CaptureResult`, `send_input(...)`, `uia_tree(...)`); nunca JSON.
- `cu-core` no importa `windows` directamente (salvo tipos compartidos que se declaren en un submódulo `types`).
- Toda frontera con el usuario/LLM serializa en `cu-mcp` (serde).
