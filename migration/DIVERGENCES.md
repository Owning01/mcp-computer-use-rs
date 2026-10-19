# Divergencias F6 — computer-use2-rs vs origen Node

Estado: **67 de 68 tools servidos nativamente** por el puente Rust. Este documento lista
exactamente qué queda fuera del camino nativo y qué implica en el cutover.

Fecha: 2026-10-19 · Gates: smoke congelado PASS (68 tools) + juez de paridad 15/15.

## 1. Tool nunca nativo

| Tool | Motivo | Qué hace el puente al recibirlo |
|---|---|---|
| `computer_upload_file` | Su Tier 1 es inyección CDP (conexión DevTools del navegador) que el origen ejecuta antes que el diálogo de archivo. Portarlo exige un cliente CDP completo. | Proxea al hijo Node (comportamiento idéntico al actual). |

## 2. Tools nativos solo en parte (por `accepts`)

Cuando los args caen fuera del subconjunto replicado, el puente **proxea** al hijo: cero
regresión, pero esa rama sigue necesitando Node.

| Grupo | Args que se proxean |
|---|---|
| input (`cu-input`) | `behavior` ausente o `auto` (el origen lo resuelve por ventana/UIA/`COMPUTER_USE_BEHAVIOR`); `headless`; `id`/`ref`/`name`/`automation_id`/`target_text`; `snapshot_after`; `COMPUTER_USE_MODE=harness\|auto`; `type` con saltos de línea/tabs; coordenadas no enteras o fuera de ±100000; `target_window` de un gestor de contraseñas |
| UIA (`cu-uia`) | `ref`/`id` (estado del proceso Node); `observe` con `screenshot=true`; `toast_action` en `list`/`dismiss`/`clear_all`; `system_tray` cuando pide overflow; `wait_stable` en su forma async/tasks; `act` con `behavior:humanized` o acciones fuera del subconjunto (drag, hover) |
| login (`cu-winops`) | rama `account` (vía CDP) |
| misc (`cu-misc`) | `isolated_desktop=true` |

## 3. Cosas que siguen necesitando el hijo Node

- Los tools y ramas de la sección 1 y 2.
- La extensión **tasks** (`tasks/list`, `tasks/get`, `tasks/result`, `tasks/cancel`) y las
  formas async de `computer_wait_stable`: el puente no implementa el protocolo de tasks.
- `resources/*` y cualquier método MCP que el puente no intercepte.

El supervisor Python de PaddleOCR (vendorizado en `crates/cu-ocr/python/`) es opcional: si
el Python no tiene Paddle, el OCR cae al fallback WinRT nativo con la misma forma de
respuesta.

## 4. Divergencias de comportamiento aceptadas (documentadas con PORT NOTE)

- **Trayectoria de clicks humanizados**: el RNG propio no reproduce la trayectoria
  intermedia ni los tiempos del origen (el punto final y el JSON coinciden).
- **`appendRunLog`/`appendError`**: la bitácora de corrida la escribe el harness, no el
  puente.
- **Ruta daemon de `computer_batch`**: no se porta; siempre se usa el fallback nativo
  (igual que el origen cuando su API local no está disponible).
- **Enriquecimiento `__element`** al grabar macros: el origen lo agrega a los clicks con
  `id`/`ref` (UIA del proceso Node); el puente graba los args saneados sin ese campo.
- **Healing de macros por UIA**: el replay nativo pide los elementos al hijo, con lo que el
  healing conserva el comportamiento del origen.

## 5. Implicancia del cutover (F8)

El binario Rust **no reemplaza al origen todavía**: es un proxy que sirve 67 tools de forma
nativa y delega el resto. Por eso:

- `G:\Proyectos\MCP\computer-use2` debe seguir en disco (el hijo lo necesita).
- Apagar el hijo (`PARITY_DISABLE_CHILD=1`) deja de servir `computer_upload_file`, las
  ramas listadas en la sección 2 y la extensión tasks.
- La ruta de cierre total del proyecto Node queda condicionada a portar CDP
  (`upload_file`, `login account`) y el protocolo tasks, o a aceptar esas ramas como
  divergencia permanente.
