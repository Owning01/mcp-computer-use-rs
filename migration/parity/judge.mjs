#!/usr/bin/env node
// judge.mjs — F2/F7 (repo-migrate): juez de paridad para servidores MCP por stdio.
//
// Compara dos servidores (A=origen, B=destino) paso a paso segun un scenario JSON.
// Uso: node judge.mjs <scenario.json> [--a-cmd '<json array>'] [--b-cmd '<json array>']
//                                [--a-cwd <dir>] [--b-cwd <dir>] [--skip id1,id2]
//
// F2: A=B=origen -> PASS (valida que el juez no tiene falsos positivos).
//     A=origen, B=mutante deliberado -> FAIL (el juez atrapa la rotura).
// Modos de comparacion por step: exact | volatile (default) | drop | shape.
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";

const args = process.argv.slice(2);
const scenarioPath = args[0];
if (!scenarioPath) { console.error("uso: node judge.mjs <scenario.json> [--a-cmd ...] [--b-cmd ...] [--skip ids]"); process.exit(2); }
const flag = (name) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : null; };
const sc = JSON.parse(readFileSync(scenarioPath, "utf8"));
const skips = new Set((flag("--skip") || "").split(",").map((s) => s.trim()).filter(Boolean));

const mkCfg = (key) => {
  const base = sc[key] || {};
  const cmd = flag(`--${key}-cmd`) ? JSON.parse(flag(`--${key}-cmd`)) : base.cmd;
  const cwd = flag(`--${key}-cwd`) || base.cwd;
  if (!Array.isArray(cmd) || cmd.length === 0) { console.error(`scenario: falta ${key}.cmd`); process.exit(2); }
  return { name: base.name || key, cmd, cwd, env: base.env || {} };
};
const A = mkCfg("a");
const B = mkCfg("b");

function startServer(cfg) {
  const child = spawn(cfg.cmd[0], cfg.cmd.slice(1), {
    cwd: cfg.cwd || process.cwd(),
    env: { ...process.env, ...cfg.env },
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  let buf = ""; let seq = 0; let stderr = "";
  const pending = new Map();
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buf += chunk;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (!line) continue;
      let msg; try { msg = JSON.parse(line); } catch { continue; }
      if (msg.id !== undefined && pending.has(msg.id)) { const r = pending.get(msg.id); pending.delete(msg.id); r(msg); }
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (d) => { stderr += d; });
  const rpc = (method, params, timeoutMs = 20000) => new Promise((resolve, reject) => {
    const id = ++seq;
    const payload = { jsonrpc: "2.0", id, method };
    if (params !== undefined) payload.params = params;
    const t = setTimeout(() => { pending.delete(id); reject(new Error(`timeout ${method} (${timeoutMs}ms) [stderr: ${stderr.slice(-400)}]`)); }, timeoutMs);
    pending.set(id, (m) => { clearTimeout(t); resolve(m); });
    child.stdin.write(JSON.stringify(payload) + "\n");
  });
  return { name: cfg.name, child, rpc, stderrRef: () => stderr };
}

// ---------------------------------------------------------------- normalizacion
const DEFAULT_DROP = [/^_meta$/, /^timestamp$/, /^took_ms$/, /^duration_ms$/, /^elapsed/i];
const vol = (pat) => new RegExp(pat.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));

function strip(value, drops) {
  if (Array.isArray(value)) return value.map((v) => strip(v, drops));
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      if (drops.some((re) => re.test(k))) continue;
      out[k] = strip(v, drops);
    }
    return out;
  }
  return value;
}

function diffValues(a, b, path, mode) {
  if (mode === "shape") {
    const ta = a === null ? "null" : Array.isArray(a) ? "array" : typeof a;
    const tb = b === null ? "null" : Array.isArray(b) ? "array" : typeof b;
    if (ta !== tb) return `${path}: tipo ${ta} != ${tb}`;
    if (ta === "array") {
      if (a.length !== b.length) return `${path}: array len ${a.length} != ${b.length}`;
      for (let i = 0; i < a.length; i++) { const d = diffValues(a[i], b[i], `${path}[${i}]`, mode); if (d) return d; }
      return null;
    }
    if (ta === "object") {
      const ka = Object.keys(a).sort(), kb = Object.keys(b).sort();
      if (ka.join("|") !== kb.join("|")) return `${path}: keys [${ka}] != [${kb}]`;
      for (const k of ka) { const d = diffValues(a[k], b[k], `${path}.${k}`, mode); if (d) return d; }
    }
    return null;
  }
  const ja = JSON.stringify(a), jb = JSON.stringify(b);
  if (ja !== jb) return `${path}: ${String(ja).slice(0, 220)} != ${String(jb).slice(0, 220)}`;
  return null;
}

function compare(step, ra, rb) {
  const mode = step.compare || "volatile";
  const drops = [];
  if (mode !== "exact") drops.push(...DEFAULT_DROP);
  for (const k of step.ignoreKeys || []) drops.push(vol(k));
  const a = mode === "exact" ? ra : strip(ra, drops);
  const b = mode === "exact" ? rb : strip(rb, drops);
  return diffValues(a, b, "$", mode === "shape" ? "shape" : "values");
}

// ---------------------------------------------------------------- ejecucion
async function execStep(srv, step) {
  const t = step.timeoutMs || 20000;
  if (step.tool) {
    const res = await srv.rpc("tools/call", { name: step.tool, arguments: step.arguments || {} }, t);
    if (res.error) return { rpcError: res.error };
    const result = res.result || {};
    const textBlock = (result.content || []).find((c) => c.type === "text" && typeof c.text === "string");
    let payload = textBlock ? textBlock.text : null;
    if (payload !== null) { try { payload = JSON.parse(payload); } catch { /* texto crudo */ } }
    return { isError: result.isError === true, payload, structured: result.structuredContent ?? null };
  }
  if (step.method === "initialize") {
    const res = await srv.rpc("initialize", step.params, t);
    // handshake: completar con la notificacion initialized
    srv.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized", params: {} }) + "\n");
    return res.error ? { rpcError: res.error } : { result: res.result ?? null };
  }
  const res = await srv.rpc(step.method, step.params, t);
  return res.error ? { rpcError: res.error } : { result: res.result ?? null };
}

const sa = startServer(A);
const sb = startServer(B);
let failures = 0;

try {
  for (const step of sc.steps || []) {
    const id = step.id || step.method || step.tool;
    if (skips.has(id)) { console.log(`SKIP ${id}`); continue; }
    try {
      const ra = await execStep(sa, step);
      const rb = await execStep(sb, step);
      const d = compare(step, ra, rb);
      if (d) { failures++; console.log(`FAIL ${id} :: ${d}`); }
      else console.log(`PASS ${id} (${step.compare || "volatile"})`);
    } catch (e) {
      failures++;
      console.log(`FAIL ${id} :: error de ejecucion: ${e.message}`);
    }
  }
} finally {
  try { sa.child.kill(); } catch {}
  try { sb.child.kill(); } catch {}
}

console.log(`\njudge: ${failures === 0 ? "PASS" : "FAIL"} (${(sc.steps || []).length} steps, ${failures} fallos) — A=${A.name} B=${B.name}`);
process.exit(failures === 0 ? 0 : 1);
