// The Chrome overlay's browser test fixture: a daemon of its own, a real
// Vite dev server on a copy of live-site/, and Chromium with the unpacked
// test build of the extension in a fresh profile, whose NativeMessagingHosts
// directory `clax extension install` registered for that daemon's home. The
// extension is loaded from `<home>/extension`, where the install puts it, so
// Chromium derives its ID from the path, as for a person's Load unpacked
// (spec L15).
import { type BrowserContext, chromium, type Worker, test as base } from "@playwright/test";
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { type AddressInfo, createServer as createNetServer } from "node:net";
import { createHash } from "node:crypto";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { type Daemon, NO_KEY_CONFIG, startDaemon } from "./fixtures";

const web = fileURLToPath(new URL("..", import.meta.url));
export const EXT_DIR = join(web, "dist-extension-test");

/** Which build Chromium loads: the test build as made (`<all_urls>`),
 * that build holding only the dev server's origin, as a person's grant
 * leaves the release build ("origin"), or holding no site at all and
 * unable to ask for one, as when the person refused the prompt ("none"). */
export type Variant = "all-urls" | "origin" | "none";

export type Live = {
  daemon: Daemon; ctx: BrowserContext; sw: Worker; extId: string; site: ViteDevServer; siteDir: string; siteUrl: string;
  /** The extension's ID the daemon admits (`GET /api/extension`). */
  daemonExtId: string;
  /** Stops the daemon and starts another on the same home and a new port. */
  restartDaemon(): Promise<void>;
  /** Closes Chromium and starts it again on the same profile (a browser restart). */
  restartBrowser(): Promise<void>;
  /** A real click on the extension's toolbar icon in the tab showing
   * `url`, which grants activeTab (the `gesture` option only). */
  iconClick(url: string): Promise<void>;
};

/** A loopback port free a moment ago. */
export async function freePort(): Promise<number> {
  const srv = createNetServer();
  await new Promise<void>(r => srv.listen(0, "127.0.0.1", r));
  const { port } = srv.address() as AddressInfo;
  await new Promise(r => srv.close(r));
  return port;
}

/** Chromium's flags for `Extensions.triggerAction` (a real toolbar click
 * over CDP): the extensions domain, on a browser-wide endpoint of its own. */
const GESTURE_ARGS = ["--enable-unsafe-extension-debugging", "--remote-debugging-port=0"];

async function launch(profile: string, extDir: string, gesture: boolean): Promise<{ ctx: BrowserContext; sw: Worker }> {
  const ctx = await chromium.launchPersistentContext(profile, {
    channel: "chromium",
    args: [`--disable-extensions-except=${extDir}`, `--load-extension=${extDir}`, ...(gesture ? GESTURE_ARGS : [])],
  });
  const sw = ctx.serviceWorkers()[0] ?? (await ctx.waitForEvent("serviceworker"));
  return { ctx, sw };
}

/** How long a daemon of these tests may take to start: each test launches a
 * daemon and a browser of its own, often right after the binary was built,
 * when its first runs are slow (macOS scans a new binary at its first exec). */
const DAEMON_START_MS = 60_000;

/** This worker's Clax home, the same path for each of its tests (emptied
 * before each), so the launcher `clax extension install` writes there, which
 * names the home, has the same text in every test. */
const WORKER_HOME = join(tmpdir(), `clax-e2e-ext-${process.pid}`);

/** Where `keep` stores the host scripts, by content. */
const KEPT = join(tmpdir(), "clax-e2e-host");

/**
 * Replaces the executable `file` with a symbolic link to a kept copy of the
 * same bytes, made on first use, and says whether that copy is new. macOS
 * assesses each new executable file on its first run, which on a loaded
 * machine can take a minute or more, and Chrome would run the freshly
 * installed host scripts (`launch.sh`, then `ensure-clax.sh`) for the first
 * time in each test's pairing; a run through a link to an assessed file is
 * not a first run (scripts/fake-exe.sh). The bytes run are the installed
 * ones: the copy is kept under their hash.
 */
function keep(file: string): boolean {
  const bytes = readFileSync(file);
  const dir = join(KEPT, createHash("sha256").update(bytes).digest("hex"));
  const kept = join(dir, "script");
  const fresh = !existsSync(kept);
  if (fresh) {
    mkdirSync(KEPT, { recursive: true });
    const staging = mkdtempSync(join(KEPT, ".new-"));
    writeFileSync(join(staging, "script"), bytes);
    chmodSync(join(staging, "script"), 0o555);
    // Another worker may have kept the same bytes meanwhile; either copy will do.
    try { renameSync(staging, dir); } catch { rmSync(staging, { recursive: true, force: true }); }
  }
  rmSync(file);
  symlinkSync(kept, file);
  return fresh;
}

/** How long one CDP command to the browser endpoint may take, connecting included. */
const CDP_MS = 10_000;

/** Sends one CDP command to the browser endpoint Chromium wrote to the
 * profile's DevToolsActivePort; fails after CDP_MS, or when the socket
 * errs or closes before the answer. */
async function browserCdp(profile: string, method: string, params: object): Promise<Record<string, unknown>> {
  const [port, path] = readFileSync(join(profile, "DevToolsActivePort"), "utf8").trim().split("\n");
  const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await new Promise((ok, fail) => {
      timer = setTimeout(() => fail(new Error(`${method}: no answer in ${CDP_MS} ms`)), CDP_MS);
      ws.onerror = () => fail(new Error(`${method}: the browser endpoint's socket failed`));
      ws.onclose = () => fail(new Error(`${method}: the browser endpoint closed before answering`));
      ws.onopen = () => ws.send(JSON.stringify({ id: 1, method, params }));
      ws.onmessage = e => {
        const m = JSON.parse(String(e.data)) as { id?: number; result?: Record<string, unknown>; error?: unknown };
        if (m.id !== 1) return;
        if (m.error) fail(new Error(`${method}: ${JSON.stringify(m.error)}`));
        else ok(m.result ?? {});
      };
    });
  } finally {
    clearTimeout(timer);
    ws.onclose = null;
    ws.close();
  }
}

export const test = base.extend<{ live: Live; variant: Variant; gesture: boolean }, { warm: void }>({
  // Once per worker, before any daemon starts: the binary's first run, so
  // the first-exec scan is not paid inside a daemon's start.
  // oxlint-disable-next-line no-empty-pattern
  warm: [async ({}, use) => {
    const r = spawnSync(process.env.CLAX_E2E_BIN!, ["--version"], { encoding: "utf8", timeout: DAEMON_START_MS });
    if (r.status !== 0) throw new Error(`clax --version failed (${r.status ?? r.signal}): ${r.stderr}`);
    await use();
  }, { scope: "worker", auto: true, timeout: DAEMON_START_MS + 5000 }],
  variant: ["all-urls", { option: true }],
  /** Chromium takes real toolbar clicks over CDP (`iconClick`). */
  gesture: [false, { option: true }],
  live: async ({ variant, gesture }, use, testInfo) => {
    // The daemon's start has its own bound, beyond the test's.
    testInfo.setTimeout(testInfo.timeout + DAEMON_START_MS);
    if (!existsSync(join(EXT_DIR, "manifest.json"))) throw new Error("web/dist-extension-test is missing: run `npm run build` in web/");
    // The native host's wrapper runs the `bin` setting's clax: this run's.
    const config = `bin = ${JSON.stringify(process.env.CLAX_E2E_BIN)}\n${NO_KEY_CONFIG}`;
    rmSync(WORKER_HOME, { recursive: true, force: true });
    mkdirSync(WORKER_HOME, { mode: 0o700 });
    writeFileSync(join(WORKER_HOME, "config.toml"), config);
    // `clax extension install`, as `clax init` runs it, registers this
    // profile's host (its launcher and manifest under <home>/extension/host);
    // the test build then replaces the release build's files beside them.
    const profile = mkdtempSync(join(tmpdir(), "clax-chrome-"));
    const hostDir = join(profile, "NativeMessagingHosts");
    mkdirSync(hostDir, { recursive: true });
    const install = spawnSync(process.env.CLAX_E2E_BIN!, ["extension", "install", "--json"], { env: { ...process.env, CLAX_HOME: WORKER_HOME, CLAX_NATIVE_HOST_DIRS: `chromium=${hostDir}` }, encoding: "utf8" });
    if (install.status !== 0) throw new Error(`clax extension install: ${install.stderr}`);
    const host = join(WORKER_HOME, "extension", "host");
    if ([keep(join(host, "launch.sh")), keep(join(host, "ensure-clax.sh"))].some(Boolean)) {
      // New copies run once here, before the daemon and outside the test's
      // waits. With no origin the host only answers wrong_origin; the logs
      // directory it may make goes, so the test starts on a home as before.
      spawnSync(join(host, "launch.sh"), [], { stdio: "ignore", timeout: DAEMON_START_MS });
      rmSync(join(WORKER_HOME, "logs"), { recursive: true, force: true });
    }
    const daemon = await startDaemon({ home: WORKER_HOME, config, startMs: DAEMON_START_MS });
    // The real path: macOS reports file changes under /private/var, not /var.
    const siteDir = realpathSync(mkdtempSync(join(tmpdir(), "clax-live-site-")));
    cpSync(join(web, "e2e/live-site"), siteDir, { recursive: true });
    // A port the system picks: Vite reads 0 as its default, 5173, where a person's own dev server may run.
    const site = await createServer({ root: siteDir, configFile: false, logLevel: "silent", server: { port: await freePort(), host: "127.0.0.1", strictPort: true } });
    await site.listen();
    // A file written before the watcher's first scan ends is not reported:
    // wait for it (chokidar has no public flag for a scan already done).
    // oxlint-disable-next-line no-underscore-dangle
    if (!(site.watcher as unknown as { _readyEmitted?: boolean })._readyEmitted) await once(site.watcher, "ready");
    const siteUrl = site.resolvedUrls!.local[0].replace("127.0.0.1", "localhost");
    const extDir = join(daemon.home, "extension");
    cpSync(EXT_DIR, extDir, { recursive: true });
    if (variant !== "all-urls") {
      const mf = JSON.parse(readFileSync(join(extDir, "manifest.json"), "utf8"));
      mf.host_permissions = variant === "origin" ? [`${new URL(siteUrl).origin}/*`] : [];
      if (variant === "none") mf.optional_host_permissions = [];
      writeFileSync(join(extDir, "manifest.json"), JSON.stringify(mf));
    }
    const status = await fetch(`${daemon.base}/api/extension`, { headers: { authorization: `Bearer ${daemon.token}` } }).then(r => r.json());
    const daemonExtId: string = status.extension_id;
    const first = await launch(profile, extDir, gesture);
    const l: Live = {
      daemon, ctx: first.ctx, sw: first.sw, extId: new URL(first.sw.url()).host, site, siteDir, siteUrl, daemonExtId,
      async restartDaemon() {
        await l.daemon.stop({ keepHome: true });
        l.daemon = await startDaemon({ home: l.daemon.home, config, startMs: DAEMON_START_MS });
      },
      async restartBrowser() {
        await l.ctx.close();
        const next = await launch(profile, extDir, gesture);
        l.ctx = next.ctx;
        l.sw = next.sw;
      },
      async iconClick(url) {
        if (!gesture) throw new Error("iconClick needs test.use({ gesture: true })");
        // Chromium lists tab targets only when asked for every type.
        const { targetInfos } = await browserCdp(profile, "Target.getTargets", { filter: [{}] }) as { targetInfos: { type: string; url: string; targetId: string }[] };
        const tab = targetInfos.find(t => t.type === "tab" && t.url === url);
        if (!tab) throw new Error(`no tab shows ${url}`);
        await browserCdp(profile, "Extensions.triggerAction", { id: l.extId, targetId: tab.targetId });
      },
    };
    await use(l);
    await l.ctx.close();
    await site.close().catch(() => {});
    await l.daemon.stop();
    rmSync(profile, { recursive: true, force: true });
    rmSync(siteDir, { recursive: true, force: true });
  },
});
export { expect } from "./fixtures";
