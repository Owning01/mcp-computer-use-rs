# RULEBOOK — computer-use2-rs (F3)

> Meta-regla: si dos agentes podrían responder distinto, va acá. Cada regla tiene dueño y fecha.
> Las enmiendas las propone el agente y las firma el humano. Nunca auto-aplicadas.
> Borrador v1 — 2026-10-18 (agente). Pendiente de firma.

## 0. Par y alcance

- **Origen:** JavaScript (Node 24, ESM, `@modelcontextprotocol/sdk` 1.30.0) + PowerShell 5.1 (daemon NDJSON, `runtime.ps1` 1.4k líneas) + C# (net9-windows10.0.19041: `DesktopBridge`, `NativeIndicator`, `wgc/Program`) + Python (`ocr_worker.py`, PaddleOCR).
- **Destino:** Rust 1.98 (edición 2024) · workspace cargo · `rmcp` 3.5.1 · `windows` 0.62.2 · `tokio` 1.53.3.
- **Estructura:** **rediseñar** (4 capas → workspace de 5 crates, `migration/API-MAPPING.md` §1). **Preservar:** el contrato MCP externo (68 tools, schemas, `annotations`, `outputSchema`, `structuredContent`, `resource_link`, progress, tasks extension, perfiles `agent`/`full`) y el formato en disco/entorno del estado (`COMPUTER_USE_*`, locks, incidents, macros) — compatible con instancias Node del origen durante el puente F4.
- **Fuera de scope:** PaddleOCR (decisión con spike en F4/F5), internals de CDP, perfiles de navegador del usuario, repos hermanos `computer-use` / `computer-use-standalone`.

## 1. Tipos (type-map — 5 ejes)

| Eje | Decisión |
| :--- | :--- |
| **Números** | Un campo que en JS siempre fue entero se tipa entero en Rust (`i32` coords px, `i64` hwnd y timestamps ms, `u32` tamaños) — **no `f64`** — porque JS imprime `1` y `f64` imprime `1.0` en JSON. División entera solo donde el origen la hace (`Math.floor(a/b)` → `a / b` con tipos enteros). Overflow: debug panic (aceptado; los rangos usados no llegan). |
| **Nulabilidad** | `null`/`undefined` → `Option<T>`. Clave **ausente** (equivale a `undefined`): `#[serde(skip_serializing_if = "Option::is_none")]`. `null` explícito: `Option<Option<T>>` o `Value::Null` según el campo. El JSON de salida replica presencia/ausencia de cada clave. |
| **Errores** | Tool con problema = respuesta MCP `isError: true` con `content[0].text` (JSON `{ok:false,error}` o `ERROR: …`). Fatal = arranque/protocolo. Códigos de `error` (`confirm_required`, `window_not_found`, …) **son contrato** (el smoke los verifica). Textos libres **no** son contrato byte-exacto. |
| **Concurrencia** | `tokio` multi-thread, elegido primero (rmcp lo exige). Todo async cuelga del runtime; prohibido fire-and-forget sin task vigilada. Win32 bloqueante corto → directo; UIA trees / capturas → `spawn_blocking`. Estado mutable del server → `struct ServerState` con `Arc<RwLock<…>>` (JS usaba estado en módulo; eso no se porta como globals). |
| **Memoria** | Ownership por firma: `&str`/`&Path` leer, `String`/`PathBuf` poseer. Cero `unsafe` fuera de `cu-native`; cada `unsafe` lleva comentario `SAFETY:` y revisión de 2 agentes. Esperado: poco (windows-rs cubre casi todo). |

### Toolchain

| | Origen | Destino |
| :--- | :--- | :--- |
| Build | — (Node) | `cargo build --release` |
| Packages | npm/pnpm | `cargo` (versiones `=` en Cargo.toml, `Cargo.lock` commiteado) |
| Run | `node src/index.js` | `target\release\computer-use2-rs.exe` |
| Test | `node tests/*.js` | `cargo test` + `migration/parity/oracle/contract-smoke.frozen.mjs` |
| Lint | — | `clippy` con `unwrap_used`/`expect_used` deny (excepción bootstrap contada) |
| Format | — | `rustfmt` |
| Fast-check | `contract_smoke.js` | juez `migration/parity/judge.mjs` |

## 2. Errores

- Librerías: `thiserror` (tipos de error). Binario: `anyhow` **solo** en arranque/config.
- Prohibido `unwrap()`/`expect()`/`panic!()` en rutas de tool. `grep` de `.unwrap()` = deuda visible; solo permitido en `fn main` (bootstrap, con mensaje) y tests.
- Redacción de secretos: `password` → `"***"` en incidentes (contrato verificado por smoke). Toda clave sensible (password/token/secret) se redacta por nombre, no por valor.
- `nextSteps` de error_recovery: texto libre; se preserva la estructura (claves), no la redacción exacta.

## 3. Concurrencia y runtime

- Un runtime tokio; los threads del OS se usan solo donde Windows los exige: message loop del tray, COM apartment para UIA, captura WGC.
- COM: inicialización explícita por thread (`CoInitializeEx`), modelo apartment elegido y documentado en `cu-native` (UIA tolera MTA; verificar en F4).
- Locks cross-process: mismo formato de archivo y mismas env vars que el origen (compatibilidad durante F4). Test: el juez con lock status ya lo cubre; agregar caso cruzado Node↔Rust en F6.
- Tasks extension (MCP 2026): una task = un `tokio::task` registrado; `tasks/get|cancel|result|list` leen/escriben `ServerState`. Progress notifications solo desde la task dueña.

## 4. Memoria y ownership

- Buffers de captura: `Vec<u8>` nuevo por captura (traducción primero; reutilizar = optimización post-cutover).
- Strings serializados a JSON: nunca re-asignar texto de campos existentes (nombres snake_case del contrato son inmutables).
- `Cow<'_, str>` solo si el port lo requiere; no decorativo.

## 5. Dependencias (una fila por reemplazo; versiones pineadas)

| Origen | Destino + versión | Diferencia de errores/defaults |
| :--- | :--- | :--- |
| `@modelcontextprotocol/sdk` 1.30.0 | `rmcp` 3.5.1 (`=`) | Verificar tasks/annotations/outputSchema en F4 con el smoke congelado; el smoke ES el juez. |
| daemon PowerShell NDJSON | **ELIMINADO** | Toda op de `runtime.ps1` se implementa nativa; el protocolo NDJSON no se emula. |
| C# helpers (3 archivos, compilados on-demand con dotnet/csc) | **ELIMINADOS** → `windows` 0.62.2 | Sin build on-demand; sin spawns de dotnet/csc. |
| `node:child_process` | `tokio::process` / `std::process` | Siempre `CREATE_NO_WINDOW` (`windowsHide: true` del origen). Sin shell salvo donde el origen usaba shell powershell (esos sitios pasan a API nativa o cmd directo — decisión por sitio en F4/F6). |
| `fetch` (CDP HTTP) | `reqwest` 0.13.5 | Errores de red con otro texto (no contrato). |
| WebSocket (CDP) | `tokio-tungstenite` 0.30 | Reconnect/timeout: replicar timeouts del origen. |
| `Buffer…base64` | `base64` 0.23.1 | Mismos alfabetos (`toString("base64")` = STANDARD). |
| `createHash("sha1"/"md5")` | `sha1` 0.11 + `md-5` 0.11 | Hex **minúsculas**, mismas longitudes (40 hex = contrato de fingerprint). |
| System.Drawing/GDI+ (JPEG, escalado bilineal, diff, BMP 32bpp) | `image` 0.25.10 (features `jpeg`) | Calidad JPEG default: parametrizar y fijar a la del origen (auditar en F4). Resize `Triangle` = bilineal. BMP 32bpp sin compresión para compat del pipeline. |
| OCR: PS `Windows.Media.Ocr` + `ocr_worker.py` (PaddleOCR) | `cu-ocr` con `Windows.Media.Ocr` vía `windows` + **sidecar PaddleOCR pendiente** | Decisión F4/F5 con spike comparativo (charter §3). Hasta entonces, el worker Python se invoca igual (process sidecar). |
| `setTimeout/setInterval` | `tokio::time` | — |
| `process.env.X \|\| def` | helper `env_or(def)` | En Rust `var("X")` devuelve `Ok("")` si está vacío; el helper replica la semántica JS (vacío → default). |
| `JSON.stringify(x, null, 2)` | `serde_json` 1.0.154 con feature `preserve_order` | Orden de claves: JS preserva inserción; serde default ordena alfabético → `preserve_order` obligatorio para payloads de contrato. |
| `regex` (0 lookarounds — verificado F1) | `regex` 1.13.1 | Sin `fancy-regex` salvo enmienda. |
| `tracing`/logs | `tracing` 0.1.44 + `tracing-subscriber` 0.3.23 | **stdout reservado al protocolo MCP**; logs SIEMPRE a stderr. |

Prohibido agregar dependencias fuera de esta tabla (2 candidatos → spike contra tests → enmienda firmada).

## 6. Intraducibles (rediseño, no emulación)

- **PS `Add-Type` inline C#** (metaprogramación que compila P/Invoke): se rediseña como bindings de `windows-rs` directos. No se emula el mecanismo.
- **Compilación on-demand del helper WGC** (dotnet/csc): desaparece; la captura WGC se hace in-process.
- **System tray (WinForms NotifyIcon vía PS):** rediseño con `Shell_NotifyIcon` + message loop en thread dedicado.
- **`eval`/`Function` dinámicos:** 0 usos reales (los matches `eval` de `browser_cdp.js:138` son el método CDP `eval`). Nada que rediseñar acá.
- **NDJSON daemon:** eliminado (no hay equivalente que preservar; su latencia es el benchmark a batir, no el formato).

## 7. Gemelos sintácticos (misma sintaxis/parecido, distinta semántica)

| JS | Rust | Divergencia real → test que la atrapa |
| :--- | :--- | :--- |
| `JSON.stringify(v,null,2)` | `to_string_pretty` | Orden de claves (inserción vs alfabético) → `preserve_order`; enteros `1` vs `1.0` → tipos enteros; `undefined` omite clave vs `None`→`null` → `skip_serializing_if`. Test: paridad de `qwen`/tools payloads con el juez. |
| `str.slice(a,b)` (UTF-16) | `&s[a..b]` (bytes; panics en frontera) | Truncar por caracteres: `chars().take(n)` / `chars().rev().take(n).rev()`. Test con emoji (surrogate pairs). |
| `Math.round` (mitad hacia +∞: `-1.5→-1`) | `f64::round` (mitad afuera: `-1.5→-2`) | helper `round_js()`; test con `.5` negativos. Aplica a coords/boxes (79 sitios con math en F1). |
| `toFixed(n)` | `format!("{:.n$}")` | Casos límite de representación binaria; test por sitio que use toFixed en payloads de contrato. |
| `x \|\| def` (trata `""`, `0`, `NaN` como ausentes) | `unwrap_or(def)` (solo `None`) | Auditar cada `\|\|` portado: elegir `unwrap_or` vs chequeo JS-like explícito. Test por sitio con `""`/`0`. |
| `x ?? def` (solo null/undefined) | `unwrap_or(def)` | Equivalencia directa ✓ (no confundir con la fila anterior). |
| `for (const ch of str)` (code points) | `.chars()` ✓ / `.bytes()` ✗ | Nunca `.bytes()` para truncar/medir texto visible. |
| `==` (coerción) | `==` (estricto) | Donde el origen dependía de coerción (`"1" == 1`), normalizar explícito; donde era igualdad de tipos, directo. |
| `Number.isFinite` | `f64::is_finite` | JSON no tiene `NaN`/`Infinity`: `JSON.stringify` los vuelve `null`; `serde_json` los rechaza. Campos numéricos de salida con posible no-finito → sanitizar a `null` manualmente. El comparador de paridad debe saberlo. |
| `setTimeout(fn, 0)` (macrotask) | `tokio::spawn` | Orden relativo no equivalente; si el orden importa, canal explícito. |
| `new Date().toISOString()` | `chrono` 0.4.45 | Mismo formato RFC3339 UTC con milisegundos; test por campo. |
| `JSON.parse` de payload propio | `serde_json::from_str` | Números grandes del origen (f64 pérdida) → si un campo puede exceder 2^53, llegar como string (auditar 8 hardPaths/USERPROFILE/pid). |
| spawn heredando env implícito | `Command` con env explícito | Node hereda `process.env` + overrides; Rust hereda + overrides igual solo si no se limpia — helper `spawn_env()` compartido. Test: spawn de powershell/cmd con y sin env. |

## 8. Notas de port (trazabilidad)

- Toda forma no-idiomática deliberada lleva `// PORT NOTE: <motivo>`; la Fase B (post-cutover) las quema una por una.
- Nombres de campos y códigos de error del contrato **no se renombran jamás** durante el port.

## 9. Prohibiciones del agente

- Nunca editar `migration/parity/oracle/` (tests de paridad congelados).
- Nunca elegir dependencias fuera de §5.
- Error repetido en 3 archivos → **parar** y proponer enmienda (no parche al archivo).
- Scope exacto: archivos tocados = partición asignada; desvío = rechazo del batch.
- Prohibido stub para tapar compilación; prohibido `TODO` sin ticket (`TODO(port): #N`).

## 10. Enmiendas (append-only)

| Fecha | Regla | Causa | Firma |
| :--- | :--- | :--- | :--- |
| 2026-10-18 | (v1 inicial) | — | **firmado: Octavio (chat, 2026-10-18)** |
| 2026-10-18 | Puente F4 en JSON-RPC crudo (sin rmcp): intercepta `initialize` / `tools/list` / `tools/call` de `computer_list_windows`; el resto se proxea byte a byte al origen | La SDK 1.30 del origen declara `capabilities.tasks` en forma pre-spec-final y rmcp 3.5.1 (spec 2026-07-28) usa `extensions` y no expone `tasks/list`; el puente necesita passthrough total para paridad. rmcp entra con los handlers nativos al apagar el puente (F7/F8) | pendiente |
