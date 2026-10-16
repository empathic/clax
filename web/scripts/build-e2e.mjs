// The browser tests' web UI: a copy of dist/ whose shell and bridge are built
// with the test clock (CLAX_TEST_CLOCK=1, see shell/src/clock.ts), written to
// --out <dir> (default dist-e2e). dist/ itself, which the release embeds, is
// never written. e2e/global-setup.ts runs it when the inputs change; Bazel's
// //web:dist_e2e runs it once per build of dist/.
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const web = fileURLToPath(new URL("..", import.meta.url));
const at = process.argv.indexOf("--out");
const out = resolve(web, at >= 0 ? process.argv[at + 1] : "dist-e2e");

/** Every file under `dir`, recursively. */
function files(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap(n => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p) : [p];
  });
}

if (!existsSync(join(web, "dist", "_clax", "bridge.js"))) throw new Error("dist/ holds no built web UI: run `pnpm run build` first");
rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, ".."), { recursive: true });
cpSync(join(web, "dist"), out, { recursive: true });
for (const p of ["index.html", "artifact.html", "_clax/shell", ".vite", ".gitkeep"]) rmSync(join(out, p), { recursive: true, force: true });
// As `vite build -c <config>` run in web/: each config sets its own root.
process.chdir(web);
process.env.CLAX_TEST_CLOCK = "1";
await build({ configFile: join(web, "vite.shell.config.ts"), logLevel: "warn", build: { outDir: out } });
await build({ configFile: join(web, "vite.bridge.config.ts"), logLevel: "warn", build: { outDir: join(out, "_clax") } });
const hooked = f => readFileSync(f, "utf8").includes("claxTestClock");
if (!hooked(join(out, "_clax", "bridge.js")) || !files(join(out, "_clax", "shell")).some(hooked)) throw new Error("the test build has no test clock");
