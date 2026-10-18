// FROZEN (F2, repo-migrate) — copia de computer-use2/tests/contract_smoke.js.
// Adaptaciones permitidas (unicas): (1) comando del server por PARITY_SERVER_CMD y cwd por
// PARITY_SERVER_CWD (default: origen Node); (2) wgcCacheDir inline (sin import del origen).
// El lado migrador NUNCA edita este archivo; cambios de expectativas = gate humano.
//
// Contract smoke test: spawnea src/index.js por stdio, valida initialize + tools/list
// (61 tools, incluidas snapshot/fingerprint/verify + macro record/replay + zoom +
// background_action/pass_barrier + política `behavior` de Fase F + captura WGC +
// Tasks extension v0.14.0 + lock multi-agente e incidentes RSI de v0.15.0:
// capability tasks, execution.taskSupport='optional' y flujo real async:true ->
// tools/call CreateTaskResult -> tasks/get -> tasks/result, más async + tasks/cancel).
// Cubre MCP 2026: title + annotations en las 61 tools, outputSchema en 6 tools estables,
// structuredContent (fingerprint/wait_stable/macro_list/list_windows/snapshot), resource_link
// para capturas (screenshot/zoom link:true) y notifications/progress (wait_stable con progressToken).
// Ejecuta solo tools read-only + round-trip de macro con dry_run; NUNCA clicks/teclado.
// Probes anti-bot SEGUROS: pass_barrier en modo ask_user (sin clicks; si no hay bloqueo
// responde blocked:false) y background_action con hwnd inválido (error controlado).
// Pasada J: lock status/release/force-gate con COMPUTER_USE_LOCK_DIR temporal; incidentes
// con COMPUTER_USE_INCIDENTS_DIR temporal (error controlado + redacción de secretos),
// verificando que no queden locks colgados.
// Fase 2: repite lista/structuredContent/resource_link a través del Client oficial del SDK 1.30.0
// para verificar que el SDK acepta y parsea annotations/outputSchema/structuredContent/resource_link.
// WGC: verifica enum capture incluye 'wgc' y, SOLO si el helper ya está cacheado (sin compilar
// en frío), prueba computer_screenshot {hwnd, capture:'wgc', width:200}; wgc_unavailable/wgc_*
// se aceptan como válidos porque dependen del estado del escritorio.
// Timeout total 90s; mata los child; exit 0 solo si todo pasa.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
// FROZEN: wgcCacheDir inline (antes: import desde ../src/handlers/capture.js).
function wgcCacheDir(env = process.env) {
  const override = String(env.COMPUTER_USE_WGC_CACHE_DIR || "").trim();
  if (override) return override;
  const base = String(env.LOCALAPPDATA || "").trim() || tmpdir();
  return join(base, "computer-use", "native", "wgc");
}

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const INDEX = join(ROOT, "src", "index.js");
// FROZEN: comando del server parametrizable (default: origen Node).
const ORIGIN_ROOT = process.env.PARITY_ORIGIN_ROOT || "G:/Proyectos/MCP/computer-use2";
const SERVER_CMD = process.env.PARITY_SERVER_CMD
  ? JSON.parse(process.env.PARITY_SERVER_CMD)
  : [process.execPath, join(ORIGIN_ROOT, "src", "index.js")];
const SERVER_CWD = process.env.PARITY_SERVER_CWD || ORIGIN_ROOT;
const TIMEOUT_MS = 90000;
const EXPECTED_TOOL_COUNT = 68;
const NEW_TOOLS = [
  "computer_snapshot", "computer_state_fingerprint", "computer_wait_stable", "computer_verify",
  "computer_macro_record", "computer_macro_list", "computer_macro_delete", "computer_macro_replay",
  "computer_zoom", "computer_background_action", "computer_pass_barrier",
  "computer_lock", "computer_incidents",
  "computer_awake", "computer_indicator", "computer_dismiss_overlay"
];
const OUTPUT_SCHEMA_TOOLS = [
  "computer_snapshot", "computer_state_fingerprint", "computer_wait_stable",
  "computer_verify", "computer_macro_list", "computer_list_windows"
];
// Fase F: los 6 tools de input aceptan `behavior` (alias runtime 'background' ≡ headless).
const BEHAVIOR_TOOLS = [
  "computer_click", "computer_double_click", "computer_click_text",
  "computer_type", "computer_drag", "computer_scroll"
];
const BEHAVIOR_ENUM = ["auto", "headless", "humanized", "foreground"];
// Tasks extension (v0.14.0): tools largas con execution.taskSupport='optional' + async.
const ASYNC_TOOLS = [
  "computer_wait_stable", "computer_macro_replay", "computer_pass_barrier", "computer_batch"
];
const TERMINAL_STATUSES = ["completed", "failed", "cancelled"];
// Aísla el storage de macros para no tocar %LOCALAPPDATA% del usuario.
const MACROS_DIR = mkdtempSync(join(tmpdir(), "cu_macros_smoke_"));
// Pasada J: aísla locks y journal de incidentes (el smoke provoca errores controlados).
const LOCK_DIR = mkdtempSync(join(tmpdir(), "cu_lock_smoke_"));
const INCIDENTS_DIR = mkdtempSync(join(tmpdir(), "cu_incidents_smoke_"));
const CHILD_ENV = {
  ...process.env,
  COMPUTER_USE_MODE: "native",
  COMPUTER_USE_PROFILE: "full",
  COMPUTER_USE_MACROS_DIR: MACROS_DIR,
  COMPUTER_USE_LOCK_DIR: LOCK_DIR,
  COMPUTER_USE_INCIDENTS_DIR: INCIDENTS_DIR
};

let sdkClient = null;

const child = spawn(SERVER_CMD[0], SERVER_CMD.slice(1), {
  cwd: SERVER_CWD,
  env: CHILD_ENV,
  stdio: ["pipe", "pipe", "pipe"],
  windowsHide: true
});

let buffer = "";
let stderr = "";
let nextId = 1;
let finished = false;
const pending = new Map();
const progressEvents = [];

function send(method, params) {
  const id = nextId++;
  const message = { jsonrpc: "2.0", id, method };
  if (params !== undefined) message.params = params;
  child.stdin.write(JSON.stringify(message) + "\n");
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
  });
}

function notify(method, params) {
  const message = { jsonrpc: "2.0", method };
  if (params !== undefined) message.params = params;
  child.stdin.write(JSON.stringify(message) + "\n");
}

child.stdout.setEncoding("utf8");
child.stdout.on("data", chunk => {
  buffer += chunk;
  let idx;
  while ((idx = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, idx).trim();
    buffer = buffer.slice(idx + 1);
    if (!line) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (message.method === "notifications/progress") {
      progressEvents.push(message.params || {});
    }
    const entry = pending.get(message.id);
    if (entry) {
      pending.delete(message.id);
      entry.resolve(message);
    }
  }
});

child.stderr.setEncoding("utf8");
child.stderr.on("data", chunk => {
  stderr += chunk;
});

function cleanup() {
  try { child.kill(); } catch {}
  try { sdkClient?.close?.().catch?.(() => {}); } catch {}
  try { rmSync(MACROS_DIR, { recursive: true, force: true }); } catch {}
  try { rmSync(LOCK_DIR, { recursive: true, force: true }); } catch {}
  try { rmSync(INCIDENTS_DIR, { recursive: true, force: true }); } catch {}
}

const timer = setTimeout(() => {
  if (finished) return;
  finished = true;
  console.error(`contract_smoke: FAIL — timeout after ${TIMEOUT_MS}ms`);
  console.error(stderr.slice(0, 2000));
  cleanup();
  setTimeout(() => process.exit(1), 200);
}, TIMEOUT_MS);
timer.unref?.();

async function callTool(name, args, meta) {
  const params = { name, arguments: args };
  if (meta) params._meta = meta;
  const res = await send("tools/call", params);
  if (res?.error) throw new Error(`tools/call ${name} rpc error: ${JSON.stringify(res.error).slice(0, 300)}`);
  const blocks = res?.result?.content || [];
  const textBlock = blocks.find(c => c.type === "text" && typeof c.text === "string");
  const text = textBlock?.text;
  if (typeof text !== "string") throw new Error(`tools/call ${name} returned no text content`);
  let json = null;
  try { json = JSON.parse(text); } catch {}
  return { res, text, json };
}

try {
  const initRes = await send("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "contract-smoke", version: "1.0.0" }
  });
  notify("notifications/initialized", {});
  // Tasks extension: initialize debe declarar capabilities.tasks (list/cancel + tools.call).
  const tasksCap = initRes?.result?.capabilities?.tasks;
  if (!tasksCap || !tasksCap.list || !tasksCap.cancel || !tasksCap.requests?.tools?.call) {
    throw new Error(`initialize sin capabilities.tasks completo: ${JSON.stringify(tasksCap)}`);
  }
  console.log("initialize OK — capabilities.tasks.list/cancel/requests.tools.call declaradas");

  const listRes = await send("tools/list", {});
  const tools = listRes?.result?.tools || [];
  if (tools.length !== EXPECTED_TOOL_COUNT) {
    throw new Error(`expected ${EXPECTED_TOOL_COUNT} tools, got ${tools.length}: ${tools.map(t => t.name).join(",")}`);
  }
  for (const toolName of NEW_TOOLS) {
    if (!tools.some(t => t.name === toolName)) throw new Error(`missing new tool: ${toolName}`);
  }
  // Orden determinista: el primer tool sigue siendo computer_screenshot.
  if (tools[0]?.name !== "computer_screenshot") throw new Error(`orden no determinista: primer tool ${tools[0]?.name}`);
  // MCP 2026 #1: title + annotations completas en las 59 tools.
  for (const t of tools) {
    if (typeof t.title !== "string" || t.title.length === 0) throw new Error(`${t.name} sin title`);
    const a = t.annotations;
    if (!a || ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"].some(k => typeof a[k] !== "boolean")) {
      throw new Error(`${t.name} sin annotations completas: ${JSON.stringify(a)}`);
    }
  }
  // MCP 2026 #2: outputSchema en tools de salida estable.
  for (const toolName of OUTPUT_SCHEMA_TOOLS) {
    const t = tools.find(x => x.name === toolName);
    if (!t?.outputSchema || t.outputSchema.type !== "object") throw new Error(`${toolName} sin outputSchema object`);
  }
  const snapMeta = tools.find(t => t.name === "computer_snapshot")?.annotations;
  if (snapMeta.readOnlyHint !== true || snapMeta.destructiveHint !== false || snapMeta.idempotentHint !== true) {
    throw new Error(`computer_snapshot annotations inesperadas: ${JSON.stringify(snapMeta)}`);
  }
  const delMeta = tools.find(t => t.name === "computer_macro_delete")?.annotations;
  if (delMeta.destructiveHint !== true || delMeta.readOnlyHint !== false) {
    throw new Error(`computer_macro_delete annotations inesperadas: ${JSON.stringify(delMeta)}`);
  }
  // Fase F: behavior en los 6 schemas de input, enum canónico + default auto.
  for (const toolName of BEHAVIOR_TOOLS) {
    const behaviorProp = tools.find(t => t.name === toolName)?.inputSchema?.properties?.behavior;
    if (!behaviorProp) throw new Error(`${toolName} sin inputSchema.properties.behavior`);
    if (JSON.stringify(behaviorProp.enum) !== JSON.stringify(BEHAVIOR_ENUM)) {
      throw new Error(`${toolName} behavior.enum inesperado: ${JSON.stringify(behaviorProp.enum)}`);
    }
    if (behaviorProp.default !== "auto") {
      throw new Error(`${toolName} behavior.default inesperado: ${JSON.stringify(behaviorProp.default)}`);
    }
  }
  console.log(`behavior OK — ${BEHAVIOR_TOOLS.length} schemas con enum ${BEHAVIOR_ENUM.join("|")} (default auto)`);
  // Tasks extension: las 4 tools largas exponen async en inputSchema + execution.taskSupport.
  for (const toolName of ASYNC_TOOLS) {
    const t = tools.find(x => x.name === toolName);
    if (!t?.inputSchema?.properties?.async) throw new Error(`${toolName} sin inputSchema.properties.async`);
    if (t.inputSchema.properties.async.default !== false) throw new Error(`${toolName} async.default inesperado`);
    if (t.execution?.taskSupport !== "optional") {
      throw new Error(`${toolName} sin execution.taskSupport='optional': ${JSON.stringify(t.execution)}`);
    }
  }
  const withExecution = tools.filter(t => t.execution !== undefined).map(t => t.name);
  if (withExecution.length !== ASYNC_TOOLS.length) {
    throw new Error(`execution presente en ${withExecution.length} tools (esperadas ${ASYNC_TOOLS.length}): ${withExecution.join(",")}`);
  }
  console.log(`tasks metadata OK — ${ASYNC_TOOLS.length} tools con async + execution.taskSupport=optional`);
  // Captura WGC: el enum de computer_screenshot.capture incluye wgc.
  const captureEnum = tools.find(t => t.name === "computer_screenshot")?.inputSchema?.properties?.capture?.enum || [];
  if (JSON.stringify(captureEnum) !== JSON.stringify(["auto", "gdi", "printwindow", "wgc"])) {
    throw new Error(`computer_screenshot capture.enum inesperado: ${JSON.stringify(captureEnum)}`);
  }
  console.log(`capture enum OK — ${captureEnum.join("|")}`);
  console.log(`tools/list OK — ${tools.length} tools (incluye ${NEW_TOOLS.join(", ")}); title+annotations en las 61; outputSchema en las 6 estables`);

  const fp = await callTool("computer_state_fingerprint", {});
  if (fp.res?.result?.isError) throw new Error(`computer_state_fingerprint isError: ${fp.text}`);
  if (!fp.json?.ok) throw new Error(`computer_state_fingerprint ok:false: ${fp.text}`);
  if (!/^[0-9a-f]{40}$/.test(String(fp.json.fingerprint))) throw new Error(`invalid fingerprint: ${fp.text}`);
  const fpStructured = fp.res?.result?.structuredContent;
  if (!fpStructured || fpStructured.fingerprint !== fp.json.fingerprint) {
    throw new Error(`computer_state_fingerprint sin structuredContent válido: ${JSON.stringify(fpStructured)}`);
  }
  console.log("computer_state_fingerprint OK (structuredContent)");

  const snap = await callTool("computer_snapshot", { max_results: 5 });
  if (snap.res?.result?.isError) throw new Error(`computer_snapshot isError: ${snap.text}`);
  if (!snap.json?.ok) throw new Error(`computer_snapshot ok:false: ${snap.text}`);
  if (!Array.isArray(snap.json.elements)) throw new Error(`computer_snapshot elements no es array: ${snap.text}`);
  if (!snap.json.counts || typeof snap.json.counts.total !== "number") throw new Error(`computer_snapshot sin counts: ${snap.text}`);
  if (snap.res?.result?.structuredContent?.ok !== true) throw new Error("computer_snapshot sin structuredContent ok:true");
  console.log(`computer_snapshot OK — mode=${snap.json.mode} total=${snap.json.counts.total} (structuredContent)`);

  // Progress notifications: wait_stable con _meta.progressToken debe emitir notifications/progress.
  progressEvents.length = 0;
  const stable = await callTool(
    "computer_wait_stable",
    { timeout_ms: 900, interval_ms: 100, stable_samples: 1, max_results: 10 },
    { progressToken: "smoke-progress" }
  );
  if (stable.res?.result?.isError) throw new Error(`computer_wait_stable isError: ${stable.text}`);
  if (typeof stable.json?.stable !== "boolean") throw new Error(`computer_wait_stable sin stable: ${stable.text}`);
  if (typeof stable.res?.result?.structuredContent?.stable !== "boolean") throw new Error("computer_wait_stable sin structuredContent");
  await new Promise(r => setTimeout(r, 200));
  if (!progressEvents.some(p => p.progressToken === "smoke-progress" && typeof p.progress === "number")) {
    throw new Error(`sin notifications/progress (${progressEvents.length} eventos)`);
  }
  console.log(`computer_wait_stable OK — stable=${stable.json.stable}; progress events=${progressEvents.length}`);

  // Tasks extension: async:true -> CreateTaskResult inmediato; polling tasks/get; tasks/result.
  const asyncStart = await send("tools/call", {
    name: "computer_wait_stable",
    arguments: { async: true, timeout_ms: 800, interval_ms: 300, max_results: 10 }
  });
  if (asyncStart?.error) throw new Error(`tools/call async rpc error: ${JSON.stringify(asyncStart.error).slice(0, 300)}`);
  const createdTask = asyncStart?.result?.task;
  if (!createdTask?.taskId || createdTask.status !== "working") {
    throw new Error(`async:true no devolvió CreateTaskResult working: ${JSON.stringify(asyncStart?.result).slice(0, 300)}`);
  }
  let taskState = createdTask;
  const taskDeadline = Date.now() + 15000;
  while (!TERMINAL_STATUSES.includes(taskState.status)) {
    if (Date.now() > taskDeadline) throw new Error(`tasks/get timeout: ${JSON.stringify(taskState).slice(0, 300)}`);
    await new Promise(r => setTimeout(r, 150));
    const getRes = await send("tasks/get", { taskId: createdTask.taskId });
    if (getRes?.error) throw new Error(`tasks/get rpc error: ${JSON.stringify(getRes.error).slice(0, 300)}`);
    taskState = getRes.result;
  }
  if (taskState.status !== "completed") {
    throw new Error(`async wait_stable terminal inesperado: ${JSON.stringify(taskState).slice(0, 300)}`);
  }
  const resultRes = await send("tasks/result", { taskId: createdTask.taskId });
  if (resultRes?.error) throw new Error(`tasks/result rpc error: ${JSON.stringify(resultRes.error).slice(0, 300)}`);
  const asyncText = (resultRes.result?.content || []).find(c => c.type === "text")?.text;
  const asyncPayload = JSON.parse(asyncText || "{}");
  if (typeof asyncPayload.stable !== "boolean" || resultRes.result?.structuredContent?.stable !== asyncPayload.stable) {
    throw new Error(`tasks/result payload inesperado: ${JSON.stringify(resultRes.result).slice(0, 300)}`);
  }
  const listTasks = await send("tasks/list", {});
  if (!Array.isArray(listTasks.result?.tasks) || !listTasks.result.tasks.some(t => t.taskId === createdTask.taskId)) {
    throw new Error(`tasks/list no incluye la task completada: ${JSON.stringify(listTasks.result).slice(0, 300)}`);
  }
  console.log(`tasks async OK — ${createdTask.taskId.slice(0, 8)}… working->completed; tasks/result stable=${asyncPayload.stable}; tasks/list la incluye`);

  // Tasks extension: cancel de una task larga -> cancelled (aborta el polling real).
  const cancelStart = await send("tools/call", {
    name: "computer_wait_stable",
    arguments: { async: true, timeout_ms: 5000, interval_ms: 300, max_results: 10 }
  });
  const cancelTask = cancelStart?.result?.task;
  if (!cancelTask?.taskId) throw new Error(`async cancel: sin task: ${JSON.stringify(cancelStart?.result).slice(0, 300)}`);
  const cancelRes = await send("tasks/cancel", { taskId: cancelTask.taskId });
  if (cancelRes?.error) throw new Error(`tasks/cancel rpc error: ${JSON.stringify(cancelRes.error).slice(0, 300)}`);
  if (cancelRes.result?.status !== "cancelled") {
    throw new Error(`tasks/cancel no devolvió cancelled: ${JSON.stringify(cancelRes.result).slice(0, 300)}`);
  }
  await new Promise(r => setTimeout(r, 200));
  const afterCancel = await send("tasks/get", { taskId: cancelTask.taskId });
  if (afterCancel?.result?.status !== "cancelled") {
    throw new Error(`tasks/get post-cancel inesperado: ${JSON.stringify(afterCancel?.result).slice(0, 300)}`);
  }
  console.log(`tasks cancel OK — ${cancelTask.taskId.slice(0, 8)}… tasks/cancel -> cancelled y tasks/get lo confirma`);

  // verify + list_windows: structuredContent de los 6 outputSchema (read-only).
  const verify = await callTool("computer_verify", { expect: { element: { name: "__cu_smoke_never_exists__", exists: false } }, timeout_ms: 800, interval_ms: 100 });
  if (verify.res?.result?.isError || !verify.json?.ok) throw new Error(`computer_verify: ${verify.text}`);
  if (verify.res?.result?.structuredContent?.status !== "confirmed") {
    throw new Error(`computer_verify sin structuredContent confirmed: ${JSON.stringify(verify.res?.result?.structuredContent)}`);
  }
  console.log("computer_verify OK (structuredContent)");

  const win = await callTool("computer_list_windows", {});
  if (win.res?.result?.isError || !Array.isArray(win.json)) throw new Error(`computer_list_windows: ${win.text}`);
  if (!Array.isArray(win.res?.result?.structuredContent?.windows)) throw new Error("list_windows sin structuredContent {windows}");
  console.log(`computer_list_windows OK — ${win.json.length} ventanas (structuredContent)`);

  // WGC: si el helper ya está cacheado (no compilar en frío: el smoke no debe tardar >60s),
  // probar la captura real y aceptar wgc_unavailable/wgc_* como válido (estado del escritorio).
  const wgcExe = join(wgcCacheDir(), "ComputerUseWgc.exe");
  if (existsSync(wgcExe)) {
    const wgcTarget = win.json.find(w => Number(w.hwnd) > 0 && Number(w.w) > 100 && Number(w.h) > 100);
    if (wgcTarget) {
      const wgcShot = await callTool("computer_screenshot", { hwnd: wgcTarget.hwnd, capture: "wgc", width: 200 });
      if (wgcShot.res?.result?.isError) {
        const wgcError = String(wgcShot.json?.error || "");
        if (!wgcError.startsWith("wgc_")) throw new Error(`computer_screenshot capture:wgc error inesperado: ${wgcShot.text}`);
        console.log(`computer_screenshot capture:wgc — error tolerado (${wgcError}): ${String(wgcShot.json?.message || "").slice(0, 120)}`);
      } else {
        if (wgcShot.json?.capture_used !== "wgc" || wgcShot.json?.capture_method !== "wgc") {
          throw new Error(`capture:wgc no reportó capture_used=wgc: ${wgcShot.text}`);
        }
        if (!Number.isFinite(wgcShot.json?.width) || wgcShot.json.width > 200 || wgcShot.json.width < 1) {
          throw new Error(`capture:wgc width inesperado: ${wgcShot.text}`);
        }
        const wgcImage = (wgcShot.res?.result?.content || []).find(c => c?.type === "image");
        if (!wgcImage?.data) throw new Error("capture:wgc sin imagen inline");
        console.log(`computer_screenshot capture:wgc OK — ${wgcShot.json.original_width}x${wgcShot.json.original_height} -> ${wgcShot.json.width}x${wgcShot.json.height}`);
      }
    } else {
      console.log("computer_screenshot capture:wgc — sin ventana elegible, probe omitido");
    }
  } else {
    console.log(`computer_screenshot capture:wgc — helper no cacheado en ${wgcExe}, probe omitido (no compila en frío)`);
  }

  // MCP 2026 #3: screenshot link:true -> text + resource_link (sin imagen inline).
  const shot = await callTool("computer_screenshot", { link: true, width: 200 });
  if (shot.res?.result?.isError) throw new Error(`computer_screenshot link isError: ${shot.text}`);
  const shotLink = (shot.res?.result?.content || []).find(c => c?.type === "resource_link");
  if (!shotLink) throw new Error("computer_screenshot link:true sin resource_link");
  if (!/^file:\/\//.test(String(shotLink.uri))) throw new Error(`resource_link uri no file://: ${shotLink.uri}`);
  if (!/^image\//.test(String(shotLink.mimeType))) throw new Error(`resource_link mimeType inválido: ${shotLink.mimeType}`);
  if (!/^image\//.test(String(shot.json?.mime)) || !shot.json?.path) throw new Error(`meta de link incompleta: ${shot.text}`);
  try { unlinkSync(shot.json.path); } catch {}
  console.log(`computer_screenshot link:true OK — ${shotLink.name} ${shotLink.mimeType}`);

  // Zoom: región nativa escalada 2x (read-only).
  const zoom = await callTool("computer_zoom", { x: 0, y: 0, w: 80, h: 60, scale: 2 });
  if (zoom.res?.result?.isError) throw new Error(`computer_zoom isError: ${zoom.text}`);
  if (!zoom.json?.ok || zoom.json.scale !== 2 || zoom.json.width !== 160 || zoom.json.height !== 120) {
    throw new Error(`computer_zoom meta inesperada: ${zoom.text}`);
  }
  const zoomImg = (zoom.res?.result?.content || []).find(c => c?.type === "image");
  if (!zoomImg) throw new Error("computer_zoom sin imagen inline");
  console.log(`computer_zoom OK — ${zoom.json.width}x${zoom.json.height} scale=${zoom.json.scale}`);

  // Anti-bot #1: pass_barrier en modo ask_user (detect-only, sin clicks/teclado).
  // Sin bloqueo => blocked:false; con bloqueo => alerta + handoff, nunca input.
  const barrierProbe = await callTool("computer_pass_barrier", { strategy: "ask_user", max_attempts: 1, timeout_ms: 2000 });
  if (!barrierProbe.json || typeof barrierProbe.json.blocked !== "boolean") {
    throw new Error(`computer_pass_barrier sin blocked booleano: ${barrierProbe.text}`);
  }
  if (barrierProbe.json.ok === false) throw new Error(`computer_pass_barrier ok:false: ${barrierProbe.text}`);
  if (barrierProbe.json.blocked === false && barrierProbe.json.type !== "none") {
    throw new Error(`pass_barrier sin bloqueo debería type none: ${barrierProbe.text}`);
  }
  console.log(`computer_pass_barrier OK — blocked=${barrierProbe.json.blocked} type=${barrierProbe.json.type} strategy=${barrierProbe.json.strategy_used}`);

  // Anti-bot #2: background_action con hwnd inválido => error controlado, sin ejecutar nada.
  // Pasada J: además genera un incidente (journal temporal) con un secreto que debe redactarse.
  const bgProbe = await callTool("computer_background_action", { action: "invoke", hwnd: 1, name: "__cu_smoke_never__", password: "smoke-secret" });
  if (bgProbe.json?.ok !== false || typeof bgProbe.json?.error !== "string") {
    throw new Error(`background_action con hwnd inválido debería fallar controlado: ${bgProbe.text}`);
  }
  if (bgProbe.res?.result?.isError !== true) throw new Error(`background_action hwnd inválido sin isError: ${bgProbe.text}`);
  console.log(`computer_background_action OK — error controlado: ${bgProbe.json.error}`);

  // Pasada J #1: lock multi-agente — status limpio, force exige confirm:true, release no-op.
  const lockStatus = await callTool("computer_lock", { action: "status" });
  if (lockStatus.res?.result?.isError || lockStatus.json?.ok !== true || !Array.isArray(lockStatus.json.locks)) {
    throw new Error(`computer_lock status: ${lockStatus.text}`);
  }
  const lockForceNoConfirm = await callTool("computer_lock", { action: "force", scope: "desktop" });
  if (lockForceNoConfirm.json?.ok !== false || lockForceNoConfirm.json?.error !== "confirm_required") {
    throw new Error(`computer_lock force sin confirm debería fallar: ${lockForceNoConfirm.text}`);
  }
  const lockRelease = await callTool("computer_lock", { action: "release" });
  if (lockRelease.res?.result?.isError || lockRelease.json?.ok !== true) {
    throw new Error(`computer_lock release: ${lockRelease.text}`);
  }
  console.log(`computer_lock OK — status enabled=${lockStatus.json.enabled} locks=${lockStatus.json.locks.length}; force gate confirm_required; release no-op`);

  const awakeOn = await callTool("computer_awake", { enabled: true, reason: "contract_smoke" });
  if (awakeOn.res?.result?.isError || awakeOn.json?.ok !== true || awakeOn.json?.awake !== true) {
    throw new Error(`computer_awake enable failed: ${awakeOn.text}`);
  }
  const awakeOff = await callTool("computer_awake", { enabled: false });
  if (awakeOff.res?.result?.isError || awakeOff.json?.ok !== true || awakeOff.json?.awake !== false) {
    throw new Error(`computer_awake disable failed: ${awakeOff.text}`);
  }
  console.log("computer_awake OK — enable/disable cycle verified");

  // Indicator HUD and cursor verification
  const indShow = await callTool("computer_indicator", { action: "show", message: "Smoke Test Active", duration_ms: 1000 });
  if (indShow.res?.result?.isError || indShow.json?.ok !== true) {
    throw new Error(`computer_indicator show failed: ${indShow.text}`);
  }
  const indHide = await callTool("computer_indicator", { action: "hide" });
  if (indHide.res?.result?.isError || indHide.json?.ok !== true) {
    throw new Error(`computer_indicator hide failed: ${indHide.text}`);
  }
  console.log("computer_indicator OK — show/hide cycle verified");

  // Overlay / Modal Dismissal verification
  const overlayTest = await callTool("computer_dismiss_overlay", { max_attempts: 1 });
  if (overlayTest.res?.result?.isError || overlayTest.json?.ok !== true) {
    throw new Error(`computer_dismiss_overlay failed: ${overlayTest.text}`);
  }
  console.log(`computer_dismiss_overlay OK — dismissed=${overlayTest.json.dismissed}`);

  // Browser Profile Discovery verification
  const profilesTest = await callTool("computer_login", { list_profiles: true });
  if (profilesTest.res?.result?.isError || profilesTest.json?.ok !== true || !Array.isArray(profilesTest.json.profiles)) {
    throw new Error(`computer_login list_profiles failed: ${profilesTest.text}`);
  }
  console.log(`computer_login list_profiles OK — ${profilesTest.json.count} profiles detected`);

  // Pasada J #2: incidentes — el error controlado quedó registrado con args redactados
  // (password -> ***), sin stack en list, con error_class; summary/export funcionan.
  const incList = await callTool("computer_incidents", { action: "list", limit: 10 });
  if (incList.res?.result?.isError || incList.json?.ok !== true || !Array.isArray(incList.json.incidents)) {
    throw new Error(`computer_incidents list: ${incList.text}`);
  }
  const bgIncident = incList.json.incidents.find(i => i.tool === "computer_background_action");
  if (!bgIncident) throw new Error(`sin incidente de computer_background_action: ${incList.text}`);
  if (!bgIncident.error_class) throw new Error(`incidente sin error_class: ${JSON.stringify(bgIncident)}`);
  if ("stack" in bgIncident) throw new Error("computer_incidents list no debe incluir stack");
  if (incList.text.includes("smoke-secret")) throw new Error("computer_incidents filtró el secreto en args_summary");
  const incGet = await callTool("computer_incidents", { action: "get", id: bgIncident.id });
  if (incGet.res?.result?.isError || incGet.json?.incident?.args_summary?.password !== "***") {
    throw new Error(`computer_incidents get/redacción: ${incGet.text}`);
  }
  const incSummary = await callTool("computer_incidents", { action: "summary" });
  if (incSummary.res?.result?.isError || incSummary.json?.ok !== true || !Array.isArray(incSummary.json.groups)) {
    throw new Error(`computer_incidents summary: ${incSummary.text}`);
  }
  const incExport = await callTool("computer_incidents", { action: "export", limit: 5 });
  if (incExport.res?.result?.isError || !/reporte RSI/.test(String(incExport.json?.markdown || ""))) {
    throw new Error(`computer_incidents export: ${incExport.text}`);
  }
  console.log(`computer_incidents OK — ${incList.json.count} entradas; class=${bgIncident.error_class}; redacción+summary+export`);

  // Round-trip SEGURO de macros: start -> fingerprint read-only -> stop -> list -> dry_run -> delete.
  const recStart = await callTool("computer_macro_record", { action: "start", name: "smoke roundtrip" });
  if (recStart.res?.result?.isError) throw new Error(`computer_macro_record start isError: ${recStart.text}`);
  if (!recStart.json?.ok || recStart.json.name !== "smoke-roundtrip" || recStart.json.recording !== true) {
    throw new Error(`computer_macro_record start: ${recStart.text}`);
  }
  console.log("computer_macro_record start OK");

  const fp2 = await callTool("computer_state_fingerprint", { max_results: 5 });
  if (fp2.res?.result?.isError || !fp2.json?.ok) throw new Error(`fingerprint durante grabación falló: ${fp2.text}`);

  const recStop = await callTool("computer_macro_record", { action: "stop" });
  if (recStop.res?.result?.isError) throw new Error(`computer_macro_record stop isError: ${recStop.text}`);
  if (!recStop.json?.ok || recStop.json.step_count < 1) throw new Error(`computer_macro_record stop: ${recStop.text}`);
  if (!String(recStop.json.path || "").toLowerCase().startsWith(MACROS_DIR.toLowerCase())) {
    throw new Error(`macro path fuera del dir temporal: ${recStop.text}`);
  }
  console.log(`computer_macro_record stop OK — step_count=${recStop.json.step_count}`);

  const list1 = await callTool("computer_macro_list", {});
  if (!Array.isArray(list1.json)) throw new Error(`computer_macro_list no devuelve array: ${list1.text}`);
  if (!list1.json.some(m => m.name === "smoke-roundtrip")) throw new Error(`macro_list no incluye smoke-roundtrip: ${list1.text}`);
  if (!Array.isArray(list1.res?.result?.structuredContent?.macros)) throw new Error("macro_list sin structuredContent {macros}");
  console.log("computer_macro_list OK (structuredContent)");

  const dry = await callTool("computer_macro_replay", { name: "smoke-roundtrip", dry_run: true });
  if (dry.res?.result?.isError || !dry.json?.ok || dry.json.dry_run !== true) throw new Error(`computer_macro_replay dry_run: ${dry.text}`);
  if (!Array.isArray(dry.json.steps) || dry.json.steps.some(s => s.status !== "dry")) {
    throw new Error(`computer_macro_replay dry_run ejecutó pasos: ${dry.text}`);
  }
  console.log("computer_macro_replay dry_run OK — no ejecutó pasos");

  // Progress en replay real: la macro solo contiene computer_state_fingerprint (read-only).
  progressEvents.length = 0;
  const replay = await callTool(
    "computer_macro_replay",
    { name: "smoke-roundtrip", heal: false },
    { progressToken: "smoke-replay-progress" }
  );
  if (replay.res?.result?.isError || !replay.json?.ok) throw new Error(`computer_macro_replay real: ${replay.text}`);
  await new Promise(r => setTimeout(r, 200));
  if (!progressEvents.some(p => p.progressToken === "smoke-replay-progress" && typeof p.progress === "number")) {
    throw new Error("sin notifications/progress de computer_macro_replay");
  }
  console.log(`computer_macro_replay OK — pasos=${replay.json.steps?.length}; progress events=${progressEvents.length}`);

  const delNoConfirm = await callTool("computer_macro_delete", { name: "smoke-roundtrip" });
  if (delNoConfirm.json?.ok !== false || delNoConfirm.json?.error !== "confirm_required") {
    throw new Error(`macro_delete sin confirm debería fallar: ${delNoConfirm.text}`);
  }
  const del = await callTool("computer_macro_delete", { name: "smoke-roundtrip", confirm: true });
  if (del.res?.result?.isError || del.json?.ok !== true || del.json?.deleted !== true) throw new Error(`macro_delete: ${del.text}`);
  console.log("computer_macro_delete OK (confirm gate + borrado)");

  const list2 = await callTool("computer_macro_list", {});
  if (Array.isArray(list2.json) && list2.json.some(m => m.name === "smoke-roundtrip")) throw new Error(`macro sigue presente tras delete: ${list2.text}`);

  // Pasada J: tras read-only + mutantes + replay, no deben quedar locks colgados.
  const lockAfter = await callTool("computer_lock", { action: "status" });
  if (lockAfter.res?.result?.isError || lockAfter.json?.ok !== true || lockAfter.json.locks.length !== 0) {
    throw new Error(`locks colgados tras los probes: ${lockAfter.text}`);
  }
  console.log("computer_lock status OK — sin locks colgados tras mutantes y macro replay");

  // Fase 2: el Client oficial del SDK 1.30.0 parsea annotations/outputSchema/structuredContent/resource_link.
  sdkClient = new Client({ name: "contract-smoke-sdk", version: "1.0.0" }, { capabilities: {} });
  const sdkTransport = new StdioClientTransport({
    command: SERVER_CMD[0],
    args: SERVER_CMD.slice(1),
    env: CHILD_ENV,
    stderr: "pipe"
  });
  await sdkClient.connect(sdkTransport);
  try {
    const sdkList = await sdkClient.listTools();
    if (sdkList.tools.length !== EXPECTED_TOOL_COUNT) {
      throw new Error(`sdk listTools: esperaba ${EXPECTED_TOOL_COUNT}, obtuvo ${sdkList.tools.length}`);
    }
    const sdkSnap = sdkList.tools.find(t => t.name === "computer_snapshot");
    if (!sdkSnap || typeof sdkSnap.title !== "string" || sdkSnap.annotations?.readOnlyHint !== true) {
      throw new Error("el SDK filtró title/annotations de computer_snapshot");
    }
    if (sdkSnap.outputSchema?.type !== "object") throw new Error("el SDK filtró outputSchema");
    for (const toolName of ASYNC_TOOLS) {
      const t = sdkList.tools.find(x => x.name === toolName);
      if (t?.execution?.taskSupport !== "optional") {
        throw new Error(`el SDK filtró execution.taskSupport de ${toolName}: ${JSON.stringify(t?.execution)}`);
      }
    }
    // Tasks end-to-end con el Client oficial 1.30.0: callToolStream auto-augmenta params.task
    // en tools con execution.taskSupport='optional' y hace el polling tasks/get -> tasks/result.
    let streamTaskId = null;
    let streamResult = null;
    let streamError = null;
    for await (const msg of sdkClient.experimental.tasks.callToolStream({
      name: "computer_wait_stable",
      arguments: { timeout_ms: 800, interval_ms: 300, max_results: 10 }
    })) {
      if (msg.type === "taskCreated") streamTaskId = msg.task.taskId;
      else if (msg.type === "result") streamResult = msg.result;
      else if (msg.type === "error") streamError = msg.error;
    }
    if (streamError) throw new Error(`callToolStream error: ${streamError.message}`);
    if (!streamTaskId || typeof streamResult?.structuredContent?.stable !== "boolean") {
      throw new Error(`callToolStream no completó el flujo task: taskId=${streamTaskId} result=${JSON.stringify(streamResult?.structuredContent)}`);
    }
    console.log(`SDK callToolStream tasks OK — ${String(streamTaskId).slice(0, 8)}… completed con structuredContent`);
    const sdkFp = await sdkClient.callTool({ name: "computer_state_fingerprint", arguments: {} });
    if (sdkFp.structuredContent?.ok !== true || typeof sdkFp.structuredContent.fingerprint !== "string") {
      throw new Error("el SDK no aceptó structuredContent de state_fingerprint");
    }
    const sdkZoom = await sdkClient.callTool({ name: "computer_zoom", arguments: { x: 0, y: 0, w: 64, h: 48, scale: 2, link: true } });
    const sdkLink = (sdkZoom.content || []).find(c => c.type === "resource_link");
    if (!sdkLink || !/^file:\/\//.test(String(sdkLink.uri))) throw new Error("el SDK no aceptó resource_link de computer_zoom");
    try {
      const metaText = (sdkZoom.content || []).find(c => c.type === "text");
      if (metaText) unlinkSync(JSON.parse(metaText.text).path);
    } catch {}
    console.log("SDK 1.30.0 client OK — annotations+title, outputSchema, structuredContent y resource_link parseados");
  } finally {
    try { await sdkClient.close(); } catch {}
    sdkClient = null;
  }

  finished = true;
  clearTimeout(timer);
  cleanup();
  console.log(`\ncontract_smoke: PASS (0.15.0 - ${EXPECTED_TOOL_COUNT} tools, annotations/outputSchema/structuredContent/resource_link/progress/behavior/wgc/tasks/lock/incidents)`);
  setTimeout(() => process.exit(0), 150);
} catch (e) {
  finished = true;
  clearTimeout(timer);
  console.error(`contract_smoke: FAIL — ${e?.message || e}`);
  if (stderr) console.error(stderr.slice(0, 2000));
  cleanup();
  setTimeout(() => process.exit(1), 150);
}
