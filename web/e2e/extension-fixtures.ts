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
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer, type ViteDevServer } from "vite";
import { type Daemon, NO_KEY_CONFIG, startDaemon } from "./fixtures";

const web = fileURLToPath(new URL("..", import.meta.url));
export const EXT_DIR = join(web, "dist-extension-test");

/** Which build Chromium loads: the test build as made (`<all_urls>`), or
 * that build holding only the dev server's origin, as a person's grant
 * leaves the release build ("origin"). */
export type Variant = "all-urls" | "origin";

export type Live = {
  daemon: Daemon; ctx: BrowserContext; sw: Worker; extId: string; site: ViteDevServer; siteDir: string; siteUrl: string;
  /** The extension's ID the daemon admits (`GET /api/extension`). */
  daemonExtId: string;
  /** Stops the daemon and starts another on the same home and a new port. */
  restartDaemon(): Promise<void>;
  /** Closes Chromium and starts it again on the same profile (a browser restart). */
  restartBrowser(): Promise<void>;
};

/** A loopback port free a moment ago. */
export async function freePort(): Promise<number> {
  const srv = createNetServer();
  await new Promise<void>(r => srv.listen(0, "127.0.0.1", r));
  const { port } = srv.address() as AddressInfo;
  await new Promise(r => srv.close(r));
  return port;
}

async function launch(profile: string, extDir: string): Promise<{ ctx: BrowserContext; sw: Worker }> {
  const ctx = await chromium.launchPersistentContext(profile, {
    channel: "chromium",
    args: [`--disable-extensions-except=${extDir}`, `--load-extension=${extDir}`],
  });
  const sw = ctx.serviceWorkers()[0] ?? (await ctx.waitForEvent("serviceworker"));
  return { ctx, sw };
}

/** How long a daemon of these tests may take to start: each test launches a
 * daemon and a browser of its own, often right after the binary was built,
 * when its first runs are slow (macOS scans a new binary at its first exec). */
const DAEMON_START_MS = 60_000;

export const test = base.extend<{ live: Live; variant: Variant }, { warm: void }>({
  // Once per worker, before any daemon starts: the binary's first run, so
  // the first-exec scan is not paid inside a daemon's start.
  // oxlint-disable-next-line no-empty-pattern
  warm: [async ({}, use) => {
    const r = spawnSync(process.env.CLAX_E2E_BIN!, ["--version"], { encoding: "utf8", timeout: DAEMON_START_MS });
    if (r.status !== 0) throw new Error(`clax --version failed (${r.status ?? r.signal}): ${r.stderr}`);
    await use();
  }, { scope: "worker", auto: true, timeout: DAEMON_START_MS + 5000 }],
  variant: ["all-urls", { option: true }],
  live: async ({ variant }, use, testInfo) => {
    // The daemon's start has its own bound, beyond the test's.
    testInfo.setTimeout(testInfo.timeout + DAEMON_START_MS);
    if (!existsSync(join(EXT_DIR, "manifest.json"))) throw new Error("web/dist-extension-test is missing: run `npm run build` in web/");
    // The native host's wrapper runs the `bin` setting's clax: this run's.
    const config = `bin = ${JSON.stringify(process.env.CLAX_E2E_BIN)}\n${NO_KEY_CONFIG}`;
    const daemon = await startDaemon({ config, startMs: DAEMON_START_MS });
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
    // `clax extension install`, as `clax init` runs it, registers this
    // profile's host (its launcher and manifest under <home>/extension/host);
    // the test build then replaces the release build's files beside them.
    const profile = mkdtempSync(join(tmpdir(), "clax-chrome-"));
    const hostDir = join(profile, "NativeMessagingHosts");
    mkdirSync(hostDir, { recursive: true });
    const install = spawnSync(process.env.CLAX_E2E_BIN!, ["extension", "install", "--json"], { env: { ...process.env, CLAX_HOME: daemon.home, CLAX_NATIVE_HOST_DIRS: `chromium=${hostDir}` }, encoding: "utf8" });
    if (install.status !== 0) throw new Error(`clax extension install: ${install.stderr}`);
    const extDir = join(daemon.home, "extension");
    cpSync(EXT_DIR, extDir, { recursive: true });
    if (variant === "origin") {
      const mf = JSON.parse(readFileSync(join(extDir, "manifest.json"), "utf8"));
      mf.host_permissions = [`${new URL(siteUrl).origin}/*`];
      writeFileSync(join(extDir, "manifest.json"), JSON.stringify(mf));
    }
    const status = await fetch(`${daemon.base}/api/extension`, { headers: { authorization: `Bearer ${daemon.token}` } }).then(r => r.json());
    const daemonExtId: string = status.extension_id;
    const first = await launch(profile, extDir);
    const l: Live = {
      daemon, ctx: first.ctx, sw: first.sw, extId: new URL(first.sw.url()).host, site, siteDir, siteUrl, daemonExtId,
      async restartDaemon() {
        await l.daemon.stop({ keepHome: true });
        l.daemon = await startDaemon({ home: l.daemon.home, config, startMs: DAEMON_START_MS });
      },
      async restartBrowser() {
        await l.ctx.close();
        const next = await launch(profile, extDir);
        l.ctx = next.ctx;
        l.sw = next.sw;
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
