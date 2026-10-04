// Before any worker starts: builds the daemon (daemon-setup.ts) and the
// browser tests' web UI, a copy of web/dist whose shell and bridge are built
// with the test clock (CLAX_TEST_CLOCK=1, see shell/src/clock.ts). web/dist
// itself, which the release embeds, is never written. The test build is kept
// under node_modules/.cache and rebuilt only when its inputs change. Workers
// find both through the environment.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
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
    join(web, "vite.shell.config.ts"), join(web, "vite.bridge.config.ts"), join(web, "svelte.config.js"), join(web, "package-lock.json"),
  ].sort();
  for (const f of inputs) h.update(relative(web, f)).update("\0").update(readFileSync(f)).update("\0");
  return h.digest("hex");
}

export default function globalSetup() {
  daemonSetup();

  // The bridge and its parts come from web/dist; build it when it is missing.
  if (!existsSync(join(web, "dist", "_clax", "bridge.js"))) run("npm", ["run", "build"], web);
  const out = join(web, "node_modules", ".cache", "clax-e2e-dist");
  const stamp = join(out, ".inputs");
  const hash = inputsHash();
  if (!existsSync(stamp) || readFileSync(stamp, "utf8") !== hash) {
    rmSync(out, { recursive: true, force: true });
    mkdirSync(join(out, ".."), { recursive: true });
    cpSync(join(web, "dist"), out, { recursive: true });
    for (const p of ["index.html", "artifact.html", "_clax/shell", ".vite"]) rmSync(join(out, p), { recursive: true, force: true });
    const env = { ...process.env, CLAX_TEST_CLOCK: "1" };
    run("npx", ["vite", "build", "-c", "vite.shell.config.ts", "--outDir", out, "--logLevel", "warn"], web, env);
    run("npx", ["vite", "build", "-c", "vite.bridge.config.ts", "--outDir", join(out, "_clax"), "--logLevel", "warn"], web, env);
    const hooked = (f: string) => readFileSync(f, "utf8").includes("claxTestClock");
    if (!hooked(join(out, "_clax", "bridge.js")) || !files(join(out, "_clax", "shell")).some(hooked)) throw new Error("the test build has no test clock");
    writeFileSync(stamp, hash);
  }
  process.env.CLAX_E2E_WEB_DIST = out;
}
