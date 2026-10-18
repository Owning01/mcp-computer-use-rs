#!/usr/bin/env node
// FIXTURE (F2) — shim que proxea a computer-use2 (origen) y rompe tools/list a proposito
// (renombra el primer tool). Valida que judge.mjs atrapa la rotura.
// Ademas es el prototipo minimo del puente MCP-a-MCP de F4 (interposicion por stdio).
import { spawn } from "node:child_process";

const ORIGIN_CMD = process.env.PARITY_ORIGIN_CMD
  ? JSON.parse(process.env.PARITY_ORIGIN_CMD)
  : ["node", "G:/Proyectos/MCP/computer-use2/src/index.js"];

const child = spawn(ORIGIN_CMD[0], ORIGIN_CMD.slice(1), {
  cwd: "G:/Proyectos/MCP/computer-use2",
  env: process.env,
  stdio: ["pipe", "pipe", "pipe"],
  windowsHide: true,
});

let buf = "";
child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
  buf += chunk;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    let msg = null;
    try { msg = JSON.parse(line); } catch {}
    if (msg && Array.isArray(msg.result?.tools) && msg.result.tools.length > 0) {
      const tools = msg.result.tools.map((t, idx) => (idx === 0 ? { ...t, name: `${t.name}_v2` } : t));
      process.stdout.write(JSON.stringify({ ...msg, result: { ...msg.result, tools } }) + "\n");
    } else {
      process.stdout.write(line + "\n");
    }
  }
});
child.stderr.pipe(process.stderr);
process.stdin.pipe(child.stdin);
process.on("exit", () => { try { child.kill(); } catch {} });
