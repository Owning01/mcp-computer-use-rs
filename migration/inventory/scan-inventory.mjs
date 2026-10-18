#!/usr/bin/env node
// scan-inventory.mjs — F1 (repo-migrate): inventario de un codebase origen.
// Uso: node scan-inventory.mjs <origen> <salida.json>
// Genera: modulos + grafo de imports internos + imports externos + spawns
// (child_process) + eval/dynamic-import + process.env + paths duros + refs a
// binarios/scripts + tests + orden de batches (hojas primero).
// Cada claim lleva evidencia `archivo:linea` (gate F1).
import { readdirSync, readFileSync, writeFileSync, statSync } from "node:fs";
import { join, posix } from "node:path";

const [, , rootArg, outArg] = process.argv;
if (!rootArg || !outArg) {
  console.error("uso: node scan-inventory.mjs <origen> <salida.json>");
  process.exit(2);
}
const ROOT = rootArg.replace(/[\\/]+$/, "");
const EXCL = /(^|[\\/])(node_modules|\.git|__pycache__|\.tmp|dist|build|\.venv|target)([\\/]|$)/;
const TEXT_EXT = new Set([".js", ".mjs", ".cjs", ".ts", ".ps1", ".py", ".cs", ".csproj"]);
const SKIP_FILES = /^(package-lock\.json|pnpm-lock\.yaml|bun\.lock)$/;
const cap = (s, n = 140) => (s.length > n ? s.slice(0, n) + "…" : s);

function walk(dir, acc = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (EXCL.test(p)) continue;
    if (e.isDirectory()) walk(p, acc);
    else if (!SKIP_FILES.test(e.name)) acc.push(p);
  }
  return acc;
}

const allFiles = walk(ROOT);
const rel = (p) => p.slice(ROOT.length + 1).replaceAll("\\", "/");
const textFiles = allFiles.filter((p) => TEXT_EXT.has(posix.extname(rel(p).toLowerCase())));
const fileSet = new Set(textFiles.map(rel));

const modules = [];
const edgeMap = new Map();     // from -> to  => { from, to, spec, evidence, count }
const externalImports = new Map(); // spec -> { spec, kind, evidence, count }
const spawns = [];
const dynamicRefs = [];
const envVars = new Map();     // name -> { name, count, firstEvidence }
const hardPaths = new Map();   // value -> { value, evidence }
const nativeRefs = new Map();  // value -> { value, evidence }
const tests = [];

const reInternal = /(?:from\s*|import\s*\(\s*|require\s*\(\s*)['"](\.[^'"]+)['"]/;
const reNodeBuiltin = /(?:from\s*|require\s*\(\s*)['"](node:[^'"]+)['"]/;
const rePkg = /(?:from\s*|require\s*\(\s*)['"]([^'"\s./][^'"]*)['"]/;
const reSpawn = /\b(spawn|spawnSync|exec|execSync|execFile|execFileSync)\s*\(/;
const reEval = /\beval\s*\(|new\s+Function\s*\(/;
const reDynImport = /\bimport\s*\(/;
const reEnv = /process\.env(?:\.([A-Za-z_][A-Za-z0-9_]*)|\[\s*['"]([^'"]+)['"]\s*\])/g;
const reWinPath = /['"`]([A-Za-z]:[\\/][^'"`\r\n]{2,})['"`]/g;
const reNative = /['"`]([^'"`\r\n]*\.(?:exe|ps1|py|dll|cs|csproj))['"`]/gi;

function resolveSpec(fromRel, spec) {
  const base = posix.normalize(posix.join(posix.dirname(fromRel), spec));
  for (const c of [base, base + ".js", base + ".mjs", base + ".cjs", base + "/index.js", base + "/index.mjs"]) {
    if (fileSet.has(c)) return c;
  }
  return null;
}

for (const f of textFiles) {
  const r = rel(f);
  const lines = readFileSync(f, "utf8").split(/\r?\n/);
  const kind = posix.extname(r).slice(1).toLowerCase();
  modules.push({ path: r, lines: lines.length, kind });
  if (/(^|\/)tests?\//.test(r) || /\.test\./.test(r)) tests.push({ path: r, lines: lines.length });

  const isJs = /\.(js|mjs|cjs|ts)$/.test(r);
  for (let i = 0; i < lines.length; i++) {
    const ln = i + 1;
    const line = lines[i];
    const ev = `${r}:${ln}`;

    if (isJs) {
      const mi = reInternal.exec(line);
      if (mi) {
        const resolved = resolveSpec(r, mi[1]);
        const to = resolved || mi[1];
        const key = `${r}->${to}`;
        const prev = edgeMap.get(key);
        if (prev) prev.count++;
        else edgeMap.set(key, { from: r, to, spec: mi[1], resolved: Boolean(resolved), evidence: ev, count: 1 });
      }
      const mb = reNodeBuiltin.exec(line);
      if (mb) bumpExt(mb[1], "node", ev);
      else {
        const mp = rePkg.exec(line);
        if (mp && !mp[1].startsWith("node:")) bumpExt(mp[1], "npm", ev);
      }
      const ms = reSpawn.exec(line);
      if (ms) {
        const tail = line.slice(ms.index + ms[0].length);
        const lit = /['"`]([^'"`\r\n]{1,140})['"`]/.exec(tail);
        spawns.push({ api: ms[1], arg: lit ? cap(lit[1]) : null, evidence: ev });
      }
      if (reEval.test(line)) dynamicRefs.push({ kind: "eval|Function", evidence: ev, snippet: cap(line.trim()) });
      if (reDynImport.test(line) && !reInternal.test(line) && !/import\s*\(\s*["']/.test(line)) {
        dynamicRefs.push({ kind: "dynamic-import", evidence: ev, snippet: cap(line.trim()) });
      }
      for (const m of line.matchAll(reEnv)) {
        const name = m[1] || m[2];
        const prev = envVars.get(name);
        if (prev) prev.count++;
        else envVars.set(name, { name, count: 1, firstEvidence: ev });
      }
    }
    for (const m of line.matchAll(reWinPath)) hardPaths.set(cap(m[1]), { value: cap(m[1]), evidence: ev });
    for (const m of line.matchAll(reNative)) nativeRefs.set(m[1], { value: cap(m[1]), evidence: ev });
  }
}
function bumpExt(spec, kind, ev) {
  const prev = externalImports.get(spec);
  if (prev) prev.count++;
  else externalImports.set(spec, { spec, kind, evidence: ev, count: 1 });
}

// Orden de batches: hojas (sin deps internas) primero — Kahn sobre el grafo resuelto.
const deps = new Map(); // module -> Set of deps
for (const e of edgeMap.values()) {
  if (!e.resolved) continue;
  if (!deps.has(e.from)) deps.set(e.from, new Set());
  deps.get(e.from).add(e.to);
}
const inDeg = new Map();
for (const m of modules) { if (!inDeg.has(m.path)) inDeg.set(m.path, 0); }
for (const [from, tos] of deps) for (const t of tos) inDeg.set(t, (inDeg.get(t) || 0) + 1);
const queue = [...inDeg.entries()].filter(([, d]) => d === 0).map(([m]) => m);
const order = [];
const seen = new Set();
while (queue.length) {
  const m = queue.shift();
  if (seen.has(m)) continue;
  seen.add(m);
  order.push(m);
  for (const t of deps.get(m) || []) {
    inDeg.set(t, inDeg.get(t) - 1);
    if (inDeg.get(t) === 0) queue.push(t);
  }
}
for (const m of modules) if (!seen.has(m.path)) order.push(m.path); // ciclos: al final

const binaries = allFiles
  .filter((p) => /\.(exe|dll)$/i.test(p))
  .map((p) => ({ path: rel(p), bytes: statSync(p).size }));

let declaredDeps = null;
try {
  const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8"));
  declaredDeps = { name: pkg.name, version: pkg.version, dependencies: pkg.dependencies || {}, devDependencies: pkg.devDependencies || {} };
} catch { /* sin package.json */ }

const map = {
  generatedAt: new Date().toISOString(),
  generator: "scan-inventory.mjs (repo-migrate F1)",
  root: ROOT,
  totals: {
    filesScanned: modules.length,
    linesScanned: modules.reduce((a, m) => a + m.lines, 0),
    internalEdges: edgeMap.size,
    externalSpecifiers: externalImports.size,
    spawnSites: spawns.length,
    tests: tests.length,
  },
  declaredDeps,
  modules,
  edges: [...edgeMap.values()].sort((a, b) => a.from.localeCompare(b.from) || a.to.localeCompare(b.to)),
  externalImports: [...externalImports.values()].sort((a, b) => b.count - a.count),
  spawns,
  dynamicRefs: [...new Map(dynamicRefs.map((d) => [d.evidence, d])).values()],
  envVars: [...envVars.values()].sort((a, b) => b.count - a.count),
  hardPaths: [...hardPaths.values()],
  nativeRefs: [...nativeRefs.values()],
  binaries,
  tests,
  suggestedBatchOrder: order,
};
writeFileSync(outArg, JSON.stringify(map, null, 2), "utf8");
console.log(`OK -> ${outArg}`);
console.log(JSON.stringify(map.totals));
