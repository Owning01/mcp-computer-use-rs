# PROJECT_MEMORY

## 2026-10-18 - Arranque del programa de migracion (F0/F1)
- Se inicio la migracion a Rust del MCP `G:\Proyectos\MCP\computer-use2` con la skill repo-migrate (8 fases, gates humanos).
- Creado scaffold: `migration/CHARTER.md` (borrador v1 para firma), `migration/inventory/scan-inventory.mjs` y `migration/dependency-map.json` (generado por script).
- Repo destino: `G:\Proyectos\MCP\computer-use2-rs`. Origen intacto en `G:\Proyectos\MCP\computer-use2`.
- Pendiente: diseno del puente/corte vertical en F4.
- F0 firmado en chat (2026-10-18): charter aprobado. Arranca F2 (oraculo) en paralelo con qwen-image-rs.
- PaddleOCR: decision diferida a F3 con spike; worker Python queda intacto hasta entonces.
- F2 completo (2026-10-18): `contract-smoke.frozen.mjs` PASA contra el origen (68 tools, tasks, structuredContent, resource_link, WGC real, macros, incidents). Juez validado: PASS A=B=origen; FAIL con shim mutante (tool renombrado) — el shim queda como prototipo del puente F4. Normalizacion del step `lock`: ignorar `pid`/`agent`/`dir` (difieren por proceso). Evidencia en `migration/ORACLE.md`.
- F3 borrador completo (2026-10-18): `RULEBOOK.md` (5 ejes, 13 gemelos sintacticos JS->Rust, deps pineadas: rmcp 3.5.1, windows 0.62.2, tokio 1.53.3, tokio-tungstenite 0.30, image 0.25.10, sha1/md-5, base64) + `API-MAPPING.md` (68 tools -> cu-mcp/cu-core/cu-native/cu-cdp/cu-ocr). Pendiente: firma humana (gate F3).
- Siguiente: F4 — puente MCP-a-MCP (shim de paridad ya prototipado) + corte vertical read-only, tras la firma.
