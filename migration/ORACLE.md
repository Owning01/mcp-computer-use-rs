# ORACLE — computer-use2-rs (F2)

## Suite congelada
`migration/parity/oracle/contract-smoke.frozen.mjs` — copia de `computer-use2/tests/contract_smoke.js` (627 líneas).
Adaptaciones permitidas (únicas, documentadas):
1. Comando del server parametrizable: `PARITY_SERVER_CMD` (JSON array), cwd `PARITY_SERVER_CWD`; default = origen Node.
2. `wgcCacheDir` inline (antes importaba `../src/handlers/capture.js` del origen).

Regla: el lado migrador NUNCA edita este archivo. Cambios de expectativas = gate humano.

    # contra el origen (baseline)
    node migration/parity/oracle/contract-smoke.frozen.mjs

    # contra el binario Rust (F4+)
    $env:PARITY_SERVER_CMD='["G:/Proyectos/MCP/computer-use2-rs/target/release/computer-use2-rs.exe"]'; node migration/parity/oracle/contract-smoke.frozen.mjs

Requiere `@modelcontextprotocol/sdk` en `migration/parity/node_modules` (`pnpm install` dentro de `migration/parity`).

## Juez de paridad
`migration/parity/judge.mjs` + `scenarios/cu2-basic.json`.
Steps F2: initialize, tools/list (perfil full, 68 tools), computer_lock status.
Nota de normalización: el step `lock` ignora `pid`, `agent` y `dir` (valores por proceso, no de contrato).

    # PASS esperado (juez sin falsos positivos): A=B=origen
    node migration/parity/judge.mjs migration/parity/scenarios/cu2-basic.json

    # FAIL esperado (rotura deliberada atrapada)
    node migration/parity/judge.mjs migration/parity/scenarios/cu2-basic.json --b-cmd '["node","G:/Proyectos/MCP/computer-use2-rs/migration/parity/fixtures/cu2-mutant-shim.mjs"]'

El shim mutante (`fixtures/cu2-mutant-shim.mjs`) es, además, el prototipo mínimo del puente MCP-a-MCP de F4.

## Categorización de tests del origen (27 archivos en tests/)
- **EXTERNOS** (portan como oráculo): `contract_smoke.js` → congelado arriba.
- **INTERNOS** (atados a módulos JS; NO portan como tales): 15 unit (`*_unit.test.js`), `lock_race.test.js`, `orca_patterns` → se re-expresan como tests Rust por módulo (F6/F7) y casos representativos se agregan al juez.
- **E2E/STRESS** (solo origen; no son gate automático): `e2e_sota_suite.js`, `challenger_*`, `test_live_mcp.js`, `wgc_live.test.js`, `test_*_diag.js`.
- **Interactivos reales** (clicks/teclado): se corren a mano en F5/F8.

## Evidencia de validación
Ver `PROJECT_MEMORY.md` (entrada 2026-10-18: comandos y resultados).
