# CHARTER — computer-use2-rs (F0)

> Programa: repo-migrate (8 fases, gates humanos). Este documento es el gate F0:
> sin firma del humano no hay fan-out. Borrador v1 — 2026-10-18.

## 1. Par y objetivo (1 frase)

**Par:** Node.js ESM + PowerShell 5.1 (daemon NDJSON) + C# (net9: DesktopBridge, NativeIndicator, WGC) + Python (ocr_worker) → **Rust 1.98** (rmcp + windows-rs + tokio).
**Objetivo:** el MCP `computer-use2` corre en runtime sobre un backend Rust nativo que expone los mismos 68 tools con paridad de contrato MCP (nombres, esquemas, `structuredContent`, `resource_link`, tasks extension) y latencia de acción ≤ baseline medido, sin Node ni PowerShell en la cadena de ejecución.

## 2. No-big-bang (strangler, por escrito)

- **Origen intacto:** `G:\Proyectos\MCP\computer-use2` sigue siendo el server activo en OpenCode hasta cada corte.
- **Destino:** `G:\Proyectos\MCP\computer-use2-rs` (repo git propio, scaffold F0/F1 ya creado).
- **Puente (F4):** el binario Rust expone el `tools/list` completo y, para tools aún no portadas, proxea `tools/call` al server Node viejo por stdio (MCP-a-MCP). El puente tiene dueño y se apaga por grupos.
- **Cutover por grupos, no por archivos:** (1) read-only (status/list/snapshot), (2) input, (3) captura/WGC, (4) wizards y resto. Cada apagado parcial corre la e2e completa.
- **Reversión:** restaurar `mcp.computer-use.command` en `opencode.json` (1 línea).

## 3. Qué NO se migra

- **CDP como protocolo** y los perfiles Chrome/Brave del usuario (dependencias externas).
- **APIs de Windows** y el estado del escritorio (obvio).
- **PaddleOCR:** se elimina el worker Python; el OCR va a `Windows.Media.Ocr` vía windows-rs (el fallback ya existe en el origen). Si la calidad PaddleOCR resulta imprescindible → ticket aparte, fuera de scope.
- **Carpetas hermanas** `computer-use` y `computer-use-standalone`: fuera de scope.

## 4. Orden de magnitud

Ancla to-rust.md: Bun 535k líneas ≈ 11 días / 64 agentes / USD 165k.
Origen: ~14,4k líneas JS en `src` (27 archivos; +5,4k en 27 tests) + ~1,6k C# (3 archivos) + ~1,4k PS (`runtime.ps1`) + worker OCR ~208 líneas.
Destino estimado: ~16–20k líneas Rust. **Días–semanas de calendario con 4–8 agentes en worktrees** (no 64). Costo de inferencia: decenas de USD. El compilador va en gate/daemon único, no en el loop caliente.

## 5. Riesgos (van a F3/F5)

| # | Riesgo | Nivel | Mitigación |
|---|---|---|---|
| R1 | UIA COM en Rust (`IUIAutomation` vía windows-rs) | Medio | spike en F4 (corte vertical) |
| R2 | WGC in-process en Rust (D3D11 → BMP); hoy es helper C# compilado on-demand | Alto | spike F4/F5; fallback PrintWindow ya existe |
| R3 | Tray / Toast WinRT / Audio (message loop, CoreAudio) | Medio | port directo, sin diseño nuevo |
| R4 | Tasks extension MCP 2026 en rmcp | Bajo-medio | verificar en F3; si falta, se porta el subset |
| R5 | Contrato doble (`agent`/`full` profiles) y annotations/outputSchema | Bajo | congelar contract_smoke en F2 |

## 6. Baseline a medir ANTES del primer componente (F4)

Startup del server · latencia por tool (daemon PS ≈ 7 ms) · `tools/list` (12 tools agent) · memoria RSS · suite actual verde (`contract_smoke.js` + unit) contra origen.

## 7. Oráculo (F2, resumen)

1. `tests/contract_smoke.js` congelado y parametrizado por comando de spawn — cubre 68 tools, tasks, `structuredContent`, `resource_link`, progress.
2. Tests de caracterización por tool con fixtures (escenarios controlados: notepad, explorer, WGC cache).
3. Juez validado contra rotura deliberada del origen.
4. Los unit tests JS atados a internos NO portan como tales: se re-expresan como tests Rust por módulo + casos en el oráculo externo (categorización completa en F2).

---

## Firma

Humano: ______________________  Fecha: ____________

(v1 propuesto por agente; enmiendas van a `migration/RULEBOOK.md` cuando exista, con firma aparte)
