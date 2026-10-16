// Before any worker starts: builds the daemon (daemon-setup.ts) and the
// browser tests' web UI (scripts/build-e2e.mjs: web/dist with the test clock),
// unless CLAX_E2E_WEB_DIST names one already. The test build is kept under
// node_modules/.cache and rebuilt only when its inputs change. Workers find
// both through the environment.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import daemonSetup from "./daemon-setup";

const web = fileURLToPath(new URL("..", import.meta.url));

function run(cmd: string, args: string[], cwd: string, env: NodeJS.ProcessEnv = process.env) {
  const r = spawnSync(cmd, args, { cwd, env, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} failed (${r.status ?? r.signal})`);
}

/** Every file under `dir`, recursively, skipping `skip`'s names. */
function files(dir: string, skip: Set<string> = new Set()): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap(n => {
    if (skip.has(n)) return [];
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p, skip) : [p];
  });
}

/** A hash of what the test build is made from. */
function inputsHash(): string {
  const h = createHash("sha256");
  const inputs = [
    ...files(join(web, "shell")), ...files(join(web, "bridge", "src")), ...files(join(web, "dist")),
    join(web, "vite.shell.config.ts"), join(web, "vite.bridge.config.ts"), join(web, "svelte.config.js"), join(web, "pnpm-lock.yaml"), join(web, "scripts", "build-e2e.mjs"),
  ].sort();
  for (const f of inputs) h.update(relative(web, f)).update("\0").update(readFileSync(f)).update("\0");
  return h.digest("hex");
}

export default function globalSetup() {
  daemonSetup();

  // A test build made already (Bazel's //web:dist_e2e) is used as it is.
  if (process.env.CLAX_E2E_WEB_DIST) {
    process.env.CLAX_E2E_WEB_DIST = resolve(process.env.CLAX_E2E_WEB_DIST);
    return;
  }
  // The bridge and its parts come from web/dist; build it when it is missing.
  if (!existsSync(join(web, "dist", "_clax", "bridge.js"))) run("pnpm", ["run", "build"], web);
  const out = join(web, "node_modules", ".cache", "clax-e2e-dist");
  const stamp = join(out, ".inputs");
  const hash = inputsHash();
  if (!existsSync(stamp) || readFileSync(stamp, "utf8") !== hash) {
    run("pnpm", ["run", "build:e2e", "--out", out], web);
    writeFileSync(stamp, hash);
  }
  process.env.CLAX_E2E_WEB_DIST = out;
}
