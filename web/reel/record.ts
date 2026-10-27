// Records one vignette of the sizzle reel (spec 2026-10-27-sizzle-reel-design
// §6.1):
//
//   node reel/record.ts --vignette <id> --mode fast|deterministic --out <dir>
//
// It starts what the vignette asks for (daemons on scratch homes and ports
// the system picks, never ~/.clax or 7480), launches the pinned Chromium,
// plays the vignette through the director, and writes <dir>/clip.webm (fast
// mode) or <dir>/frames/NNNNNN.png (deterministic mode, from 000000, one PNG
// of the capture's device pixels per frame), and <dir>/beats.json: {"fps":
// 30, "frames": <n>, "beats": [{"name", "frame"}]}, each beat with its first
// frame. Every daemon it started is stopped by its handle, and the browser
// closed, however the run ends.
//
// It needs CLAX_E2E_BIN (the clax binary), CLAX_E2E_WEB_DIST (the web UI
// built with the test clock, //web:dist_e2e) and PLAYWRIGHT_BROWSERS_PATH,
// as the browser tests do; under Bazel (web/reel/reel.bzl) they name files
// from the execution root, made absolute here.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { parseArgs } from "node:util";

// Node runs the recorder's TypeScript by stripping its types, and resolves
// relative imports exactly as written; the e2e helpers it reuses import each
// other without an extension, as their bundler-style resolution allows. A
// relative import that names no file is tried again as a .ts file.
registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (e) {
      if (/^\.\.?\//.test(specifier) && !/\.[cm]?[jt]s$/.test(specifier)) return nextResolve(`${specifier}.ts`, context);
      throw e;
    }
  },
});

/** Chromium's flags for the camera: no font hinting and no subpixel text, so text renders the same each run (spec §5). */
const CHROMIUM_ARGS = ["--font-render-hinting=none", "--disable-lcd-text"];

/** Chromium's flags for a deterministic camera: every compositor stage
 * (raster, decoding) done before a frame is drawn, so a screenshot never
 * shows a tile or image still on its way. */
const DETERMINISTIC_ARGS = ["--run-all-compositor-stages-before-draw", "--disable-checker-imaging", "--disable-threaded-scrolling"];

/** Math.random for a deterministic camera's documents: mulberry32 from one
 * seed, so a page drawing on it (backoff jitter, a random haiku) draws the
 * same numbers in the same order each run. */
const SEEDED_RANDOM = `(() => {
  let s = 0x2f6b1a3d;
  Math.random = () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
})()`;

function fail(message: string): never {
  console.error(`error: ${message}`);
  process.exit(2);
}

const { values: opts } = parseArgs({
  options: { vignette: { type: "string" }, mode: { type: "string" }, out: { type: "string" } },
  strict: true,
});
const id = opts.vignette ?? fail("usage: node reel/record.ts --vignette <id> --mode fast|deterministic --out <dir>");
if (!/^[a-z][a-z0-9-]*$/.test(id)) fail(`--vignette ${JSON.stringify(id)}: not a vignette ID`);
const mode = opts.mode ?? "fast";
if (mode !== "fast" && mode !== "deterministic") fail(`--mode ${JSON.stringify(mode)}: want fast or deterministic`);
const out = resolve(opts.out ?? fail("--out <dir> is required"));

// Under Bazel the paths name files from the execution root (a recording,
// web/reel/reel.bzl) or from the runfiles (a test, web/reel/BUILD.bazel).
const roots = [
  process.env.JS_BINARY__EXECROOT,
  process.env.JS_BINARY__RUNFILES && join(process.env.JS_BINARY__RUNFILES, process.env.JS_BINARY__WORKSPACE ?? "_main"),
].filter((r): r is string => !!r);
for (const name of ["CLAX_E2E_BIN", "CLAX_E2E_WEB_DIST", "PLAYWRIGHT_BROWSERS_PATH"]) {
  const value = process.env[name];
  if (!value) fail(`${name} is unset (web/reel/reel.bzl sets it under Bazel)`);
  if (!isAbsolute(value)) process.env[name] = roots.map(r => join(r, value)).find(p => existsSync(p)) ?? resolve(value);
}

// Loaded after the paths are fixed: Playwright reads its browsers' path as it loads.
const { chromium } = await import("@playwright/test");
const { parse } = await import("smol-toml");
const { parseReel } = await import("./config.ts");
const { deterministicCamera, fastCamera, FPS, REEL_EPOCH } = await import("./camera.ts");
const { installCursor } = await import("./cursor.ts");
const { Director } = await import("./director.ts");
const { NO_KEY_CONFIG, startDaemon } = await import("../e2e/fixtures.ts");
const { SECRET, freePort } = await import("../e2e/hosted-helpers.ts");
type Stage = import("./director.ts").Stage;
type Vignette = import("./director.ts").Vignette;
type Page = import("@playwright/test").Page;

// reel.toml must parse; that every vignette it lists has a script is the
// unit tests' check (config.test.ts), as one recording holds one script.
const toml = readFileSync(new URL("./reel.toml", import.meta.url), "utf8");
const listed = ((parse(toml) as { vignette?: { id?: unknown }[] }).vignette ?? []).map(v => String(v.id));
const cfg = parseReel(toml, listed);
const vcfg = cfg.vignettes.find(v => v.id === id) ?? fail(`reel.toml lists no vignette ${id}`);
const vignette = ((await import(`./vignettes/${id}.ts`)) as { default: Vignette }).default;
if (vignette.id !== id) fail(`vignettes/${id}.ts is vignette ${vignette.id}`);

const stops: (() => Promise<void> | void)[] = [];
const browser = await chromium.launch({ args: mode === "deterministic" ? [...CHROMIUM_ARGS, ...DETERMINISTIC_ARGS] : CHROMIUM_ARGS });
const videoDir = mode === "fast" ? mkdtempSync(join(tmpdir(), "clax-reel-video-")) : null;
let camera: { page: Page; startedAt: number } | null = null;

/** startDaemon, stopped when the recording ends. */
const stagedDaemon: typeof startDaemon = async (o = {}) => {
  const d = await startDaemon(o);
  stops.push(() => d.stop());
  return d;
};

const stage: Stage = {
  browser,
  startDaemon: stagedDaemon,
  // As web/e2e/hosted-helpers.ts's hostedDaemon starts one, but outside a
  // test: a daemon that took another port than its public origin names
  // fails the recording instead of skipping a test.
  async hostedDaemon(extra = "", env = {}, base = NO_KEY_CONFIG) {
    const port = await freePort();
    const origin = `http://localhost:${port}`;
    const config = `${base}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n${extra}`;
    const d = await stagedDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET, ...env } });
    if (d.port !== port) throw new Error(`the hosted daemon took port ${d.port}, not ${port}: its public origin would not match`);
    return { d, origin };
  },
  async newCamera(o = {}) {
    if (camera) throw new Error("a vignette has one camera");
    const [width, height] = cfg.capture.viewport;
    const scale = cfg.capture.scale;
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: scale,
      colorScheme: cfg.capture.theme,
      reducedMotion: "no-preference",
      locale: "en-US",
      timezoneId: "UTC",
      storageState: o.storageState,
      // At the viewport's CSS size: Playwright's video scales a page down
      // to fit, never up, so the capture's device pixels (spec §5) would
      // only pad it. Deterministic mode captures those, as screenshots.
      ...(videoDir ? { recordVideo: { dir: videoDir, size: { width, height } } } : {}),
    });
    await context.clock.install({ time: REEL_EPOCH });
    if (mode === "deterministic") await context.addInitScript(SEEDED_RANDOM);
    const page = await context.newPage();
    // The video began with the page.
    camera = { page, startedAt: Date.now() };
    await installCursor(page);
    return page;
  },
  onStop(fn) {
    stops.push(fn);
  },
};

let failed = false;
try {
  const cast = await vignette.setup(stage);
  if (!camera) throw new Error(`${id}: its setup made no camera (stage.newCamera)`);
  const { page, startedAt } = camera as { page: Page; startedAt: number };
  const beats = (cam: { beats: { name: string; frame: number }[] }) => cam.beats.map(b => `${b.name}@${b.frame}`).join(", ");
  mkdirSync(out, { recursive: true });
  if (mode === "fast") {
    const cam = fastCamera(startedAt, FPS);
    await vignette.play(new Director(page, cam, id), cast);
    const frames = cam.frame();
    const video = page.video();
    if (!video) throw new Error("the camera recorded no video");
    await page.context().close();
    await video.saveAs(join(out, "clip.webm"));
    writeFileSync(join(out, "beats.json"), `${JSON.stringify({ fps: FPS, frames, beats: cam.beats }, null, 2)}\n`);
    const seconds = (cam.elapsedMs() / 1000).toFixed(1);
    console.log(`recorded ${id} (fast, ${seconds} s, ${vcfg.seconds} s in the reel) to ${out}: clip.webm, beats.json (${beats(cam)})`);
  } else {
    // The page's time stops here and moves only as the director rolls.
    const cam = await deterministicCamera(page, out, FPS);
    await vignette.play(new Director(page, cam, id), cast);
    const frames = cam.frame();
    writeFileSync(join(out, "beats.json"), `${JSON.stringify({ fps: FPS, frames, beats: cam.beats }, null, 2)}\n`);
    const seconds = (frames / FPS).toFixed(1);
    console.log(`recorded ${id} (deterministic, ${frames} frames, ${seconds} s, ${vcfg.seconds} s in the reel) to ${out}: frames/, beats.json (${beats(cam)})`);
  }
} catch (e) {
  failed = true;
  console.error(`error: recording ${id}: ${e instanceof Error ? e.stack ?? e.message : String(e)}`);
} finally {
  for (const stop of stops.reverse()) {
    try {
      await stop();
    } catch (e) {
      console.error(`error: stopping: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  await browser.close();
  if (videoDir) rmSync(videoDir, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
