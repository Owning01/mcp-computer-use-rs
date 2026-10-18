# PROJECT_MAP - computer-use2-rs

## Que es
MCP de control de escritorio Windows: 68 tools (12 en perfil agent), UIA/Win32/WGC/CDP, locks multi-agente, tasks extension.
Reemplazo Rust del MCP origen `G:\Proyectos\MCP\computer-use2` (stack origen: Node.js ESM + PowerShell 5.1 (daemon NDJSON) + C# (net9: DesktopBridge, NativeIndicator, WGC) + Python (ocr_worker)). Programa repo-migrate, fases F0->F8.

## Estructura
- `migration/` - artefactos del programa (CHARTER.md, dependency-map.json, inventory/).
- `docs/` - documentacion del proyecto (skill mi-entorno).
- Workspace Rust: se define en F3/F4; no existe aun.

## Comandos
- Inventario F1: `node migration/inventory/scan-inventory.mjs G:\Proyectos\MCP\computer-use2 migration/dependency-map.json` (desde la raiz del repo).
- Build (cuando exista el workspace): `cargo build --release` -> `target/release/computer-use2-rs.exe`.

## Trampas
- El origen sigue siendo el server activo de OpenCode hasta el cutover; no tocar su config antes.
- Cutover y rollback son 1 linea de `opencode.json` (ver docs/04-despliegue.md).
