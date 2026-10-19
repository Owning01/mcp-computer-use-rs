#!/usr/bin/env node
// bench.mjs — F4 (repo-migrate): baseline antes/despues por servidor MCP.
// Uso: node bench.mjs '<json cmd array>' --cwd <dir> --env '<json>' --call '<tool>|<json args>' [--call ...]
// Mide: t_spawn_init_ms (spawn -> initialize respondido), rss_kb, y t_ms por tools/call.
import { spawn, execSync } from "node:child_process";

const args = process.argv.slice(2);
const cmd = JSON.parse(args[0]);
const flag = (n) => { const i = args.indexOf(n); return i >= 0 ? args[i + 1] : null; };
const cwd = flag("--cwd") || process.cwd();
const env = { ...process.env, ...(flag("--env") ? JSON.parse(flag("--env")) : {}) };
const calls = [];
for (let i = 0; i < args.length; i++) if (args[i] === "--call") calls.push(JSON.parse(args[i + 1]));

const t0 = performance.now();
const child = spawn(cmd[0], cmd.slice(1), { cwd, env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
let buf = "", id = 0;
const pending = new Map();
child.stdout.setEncoding("utf8");
child.stdout.on("data", (c) => {
  buf += c;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
    if (!line) continue;
    let m; try { m = JSON.parse(line); } catch { continue; }
    if (m.id !== undefined && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  }
});
let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", (d) => { stderr += d; });
const rpc = (method, params) => new Promise((res, rej) => {
  const rid = ++id;
  const t = setTimeout(() => rej(new Error(`timeout ${method}`)), 60000);
  pending.set(rid, (m) => { clearTimeout(t); res(m); });
  const p = { jsonrpc: "2.0", id: rid, method };
  if (params) p.params = params;
  child.stdin.write(JSON.stringify(p) + "\n");
});

const out = { cmd, measuredAt: new Date().toISOString() };
try {
  await rpc("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "bench", version: "0" } });
  out.t_spawn_init_ms = Math.round(performance.now() - t0);
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized", params: {} }) + "\n");

  try {
    const csv = execSync(`tasklist /FI "PID eq ${child.pid}" /FO CSV /NH`, { encoding: "utf8" });
    const m = csv.match(/"([\d.,]+) K"/);
    if (m) out.rss_kb = parseInt(m[1].replace(/[.,]/g, ""), 10);
  } catch { out.rss_kb = null; }

  out.calls = [];
  for (const c of calls) {
    const t = performance.now();
    const res = await rpc("tools/call", { name: c.name, arguments: c.arguments || {} });
    const dt = Math.round(performance.now() - t);
    const text = (res.result?.content || []).find((x) => x.type === "text" && typeof x.text === "string")?.text || "";
    out.calls.push({ name: c.name, t_ms: dt, isError: res.result?.isError === true, textHead: text.slice(0, 120) });
  }
} catch (e) {
  out.error = String(e.message || e);
  out.stderr = stderr.slice(-400);
} finally {
  try { child.kill(); } catch {}
}
console.log(JSON.stringify(out, null, 2));
process.exit(out.error ? 1 : 0);
