// A clax served without Clax's Content Security Policy
// (`content_security_policy: unrestricted`, PATCH /api/artifacts/<id>;
// docs/contract.md, "Security model"): at `self` (every artifact's at first)
// its page's script from another origin and its WebSocket to that origin
// are refused by the policy and never reach it; once the owner serves it
// unrestricted, both work, and a service worker of the page's own registers
// and stays registered (no longer Clax's, which unregisters itself). Then
// the shell's setting, in a hosted Clax's Share dialog
// (web/shell/src/ui/ShareDialog.svelte): turning it on shows its
// explanation and sends nothing until the owner confirms, after which the
// daemon holds `unrestricted`; turning it off returns it to `self` at once.
// Returning a clax to `self` holds in a browser that kept its files from
// while it was `unrestricted`: a page framing one of its SVG files fetches
// nowhere else through it. On a hosted Clax, a private `unrestricted`
// clax's service worker registers from a top-level visit of its site.
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { createServer as createHttpServer, type Server } from "node:http";
import { createServer, type AddressInfo } from "node:net";
import { type Browser, type BrowserType, type Frame, chromium, firefox, webkit } from "@playwright/test";
import { test, expect, type Daemon, NO_KEY_CONFIG, api, openArtifact, publish, startDaemon, trustBrowser } from "./fixtures";
import { ownerCookie } from "./questions-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

/** The engines the policy's cases run in, each skipped where Playwright has no build of it
 * (`PLAYWRIGHT_BROWSERS_PATH`): under Bazel, `@chromium//:browsers` holds Chromium alone. */
const ENGINES: BrowserType[] = [chromium, webkit, firefox];

/** Runs `body` with a browser of `engine`'s own, every context of it a browser of the owner's. */
async function inEngine(engine: BrowserType, body: (browser: Browser) => Promise<void>) {
  test.skip(!existsSync(engine.executablePath()), `${engine.name()} is not installed`);
  const browser = trustBrowser(await engine.launch());
  try {
    await body(browser);
  } finally {
    await browser.close();
  }
}

const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/** A server on another port of this host: `/lib.js` (a script that marks the
 * window) over HTTP, and an echo of one short text frame over a WebSocket at
 * `/echo`; it counts every request and upgrade it gets. */
async function elsewhere(): Promise<{ url: string; ws: string; hits: () => number; close: () => void }> {
  let hits = 0;
  const s: Server = createHttpServer((_, res) => {
    hits++;
    res.writeHead(200, { "content-type": "text/javascript" }).end("window.awayLoaded = true;");
  });
  s.on("upgrade", (req, sock) => {
    hits++;
    const key = String(req.headers["sec-websocket-key"] ?? "");
    const accept = createHash("sha1").update(key + WS_GUID).digest("base64");
    sock.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    // One masked client text frame of under 126 bytes, echoed unmasked.
    sock.once("data", (b: Buffer) => {
      const len = b[1] & 0x7f;
      const mask = b.subarray(2, 6);
      const text = Buffer.from(b.subarray(6, 6 + len).map((x, i) => x ^ mask[i % 4]));
      sock.write(Buffer.concat([Buffer.from([0x81, text.length]), text]));
    });
    sock.on("error", () => {});
  });
  await new Promise<void>(r => s.listen(0, "127.0.0.1", r));
  const host = `127.0.0.1:${(s.address() as AddressInfo).port}`;
  return { url: `http://${host}`, ws: `ws://${host}`, hits: () => hits, close: () => { s.closeAllConnections(); s.close(); } };
}

const page = (away: { url: string; ws: string }) => `<!doctype html><html><head><title>Unrestricted</title>
<script>
window.violations = [];
document.addEventListener("securitypolicyviolation", e => violations.push(e.effectiveDirective + " " + e.blockedURI));
window.socket = new Promise(r => {
  try {
    const s = new WebSocket("${away.ws}/echo");
    s.onopen = () => s.send("ping");
    s.onmessage = e => r("echo:" + e.data);
    s.onerror = () => r("refused");
  } catch (e) { r("threw:" + e.name); }
});
</script>
<script src="${away.url}/lib.js"></script>
</head><body><p>unrestricted</p></body></html>`;

/** A service worker of the page's own: it answers `hello.html` itself. */
const SW = `self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", e => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", e => { if (e.request.url.endsWith("/hello.html")) e.respondWith(new Response("from the worker", { headers: { "content-type": "text/plain" } })); });`;

type Win = { violations?: string[]; awayLoaded?: boolean; socket?: Promise<string> };
const violations = (f: Frame) => f.evaluate(() => (window as unknown as Win).violations ?? []);

/** Changes artifact `id` as the owner's browser does (`content_security_policy` and `site_access` each alone): with
 * the token alone, while an agent session of another test is live, `unrestricted` or `world` would wait for a
 * browser confirmation. */
async function patchAsOwner(base: string, id: string, change: { content_security_policy: "unrestricted" | "self" } | { site_access: "world" | "private" }) {
  const res = await fetch(`${base}/api/artifacts/${id}`, {
    method: "PATCH", headers: { cookie: ownerCookie(base), origin: base, "content-type": "application/json" },
    body: JSON.stringify(change),
  });
  const text = await res.text();
  expect(res.status, text).toBe(200);
  expect((JSON.parse(text) as { artifact: Record<string, string> }).artifact).toMatchObject(change);
}

for (const engine of ENGINES) {
  test(`${engine.name()}: a clax's page at self loads no script from elsewhere and opens no WebSocket there; once it is unrestricted, both work and its own service worker stays`, () => inEngine(engine, async browser => {
    const away = await elsewhere();
    try {
      const { artifact } = await api(d.base, d.token, "/api/artifacts", {
        method: "POST",
        body: JSON.stringify({
          title: "Unrestricted",
          files: { "index.html": { content: page(away), encoding: "utf8" }, "sw.js": { content: SW, encoding: "utf8" } },
        }),
      }) as { artifact: { id: string; content_security_policy?: string } };
      expect(artifact.content_security_policy ?? "self").toBe("self");

      // At `self`: both refused by the policy, and the other host never asked.
      const selfCtx = await browser.newContext();
      const f = await openArtifact(await selfCtx.newPage(), d.base, artifact.id, 1);
      // Refused as the socket opens, or as it is made (a browser may throw there).
      expect(await f.evaluate(() => (window as unknown as Win).socket)).toMatch(/^(refused|threw:)/);
      await expect.poll(async () => (await violations(f)).some(v => v.startsWith("script-src") && v.includes(away.url))).toBe(true);
      await expect.poll(async () => (await violations(f)).some(v => v.startsWith("connect-src ") && v.includes(away.ws))).toBe(true);
      expect(await f.evaluate(() => (window as unknown as Win).awayLoaded ?? false)).toBe(false);
      expect(away.hits()).toBe(0);
      await selfCtx.close();

      // The owner serves it unrestricted; the daemon says so.
      await patchAsOwner(d.base, artifact.id, { content_security_policy: "unrestricted" });
      const got = await api(d.base, d.token, `/api/artifacts/${artifact.id}`) as { artifact: { content_security_policy: string } };
      expect(got.artifact.content_security_policy).toBe("unrestricted");

      // A fresh browser context, so no response of the page at `self` is reused.
      const unrestrictedCtx = await browser.newContext();
      const g = await openArtifact(await unrestrictedCtx.newPage(), d.base, artifact.id, 1);
      expect(await g.evaluate(() => (window as unknown as Win).socket)).toBe("echo:ping");
      await expect.poll(() => g.evaluate(() => (window as unknown as Win).awayLoaded ?? false)).toBe(true);
      expect(await violations(g)).toEqual([]);
      expect(away.hits()).toBeGreaterThanOrEqual(2);

      // Registered from the shell's frame of a private site: in Chromium and Firefox the frame's site cookie
      // does not go with the worker's script fetch, answered 401 `site_session_required`; WebKit sends it,
      // and the worker registers there.
      const fromFrame = await g.evaluate(() => navigator.serviceWorker.register("sw.js").then(() => "registered", (e: Error) => `refused:${e.name}`));
      if (engine === webkit) expect(fromFrame).toBe("registered");
      else {
        expect(fromFrame).toMatch(/^refused:/);
        expect(await g.evaluate(async () => (await navigator.serviceWorker.getRegistrations()).length)).toBe(0);
      }

      // Its own service worker registers, answers a frame of its scope, and stays, once the site is open
      // to the world.
      await patchAsOwner(d.base, artifact.id, { site_access: "world" });
      const scriptURL = await g.evaluate(async () => {
        await navigator.serviceWorker.register("sw.js");
        const reg = await navigator.serviceWorker.ready;
        return reg.active?.scriptURL ?? "";
      });
      expect(scriptURL).toMatch(/\/sw\.js$/);
      await expect.poll(() => g.evaluate(async () => {
        const fr = document.createElement("iframe");
        fr.src = "hello.html";
        await new Promise(r => { fr.onload = r; document.body.append(fr); });
        const text = fr.contentDocument?.body?.textContent ?? "";
        fr.remove();
        return text;
      })).toBe("from the worker");
      expect(await g.evaluate(async () => (await navigator.serviceWorker.getRegistrations()).map(r => r.active?.scriptURL ?? ""))).toEqual([scriptURL]);
      await unrestrictedCtx.close();
    } finally {
      away.close();
    }
  }));
}

/** A server on another port of this host that answers every request 204 and keeps each path it is asked. */
async function listener(): Promise<{ url: string; paths: string[]; close: () => void }> {
  const paths: string[] = [];
  const s: Server = createHttpServer((req, res) => { paths.push(req.url ?? ""); res.writeHead(204).end(); });
  await new Promise<void>(r => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${(s.address() as AddressInfo).port}`, paths, close: () => { s.closeAllConnections(); s.close(); } };
}

/** A page that frames its version's `x.svg` (a document, served raw) and fetches `${to}/<label>` through the
 * framed SVG's window: "sent" when the policy let the request go, "refused" when it did not. */
const svgProbe = `<!doctype html><html><head><title>Back to self</title></head><body><p>back to self</p>
<script>
window.through = label => new Promise(resolve => {
  const fr = document.createElement("iframe");
  fr.src = "/_clax/v/1/x.svg";
  fr.onload = () => fr.contentWindow.fetch(window.to + "/" + label, { mode: "no-cors" })
    .then(() => resolve("sent"), () => resolve("refused"))
    .finally(() => fr.remove());
  document.body.append(fr);
});
window.direct = label => fetch(window.to + "/" + label, { mode: "no-cors" }).then(() => "sent", () => "refused");
</script></body></html>`;
const SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>`;

type Probe = { to?: string; through: (label: string) => Promise<string>; direct: (label: string) => Promise<string> };

for (const engine of ENGINES) {
  test(`${engine.name()}: a clax returned to self holds in a browser that kept its files from while it was unrestricted`, () => inEngine(engine, async browser => {
    const away = await listener();
    try {
      const { artifact } = await api(d.base, d.token, "/api/artifacts", {
        method: "POST",
        body: JSON.stringify({
          title: "Back to self",
          files: { "index.html": { content: svgProbe, encoding: "utf8" }, "x.svg": { content: SVG, encoding: "utf8" } },
        }),
      }) as { artifact: { id: string } };
      await patchAsOwner(d.base, artifact.id, { content_security_policy: "unrestricted" });

      // One browser context throughout, so whatever it caches while `unrestricted` is what it holds after.
      const ctx = await browser.newContext();
      const tab = await ctx.newPage();
      let f = await openArtifact(tab, d.base, artifact.id, 1);
      await f.evaluate(to => { (window as unknown as Probe).to = to; }, away.url);
      expect(await f.evaluate(() => (window as unknown as Probe).through("unrestricted-svg"))).toBe("sent");
      await expect.poll(() => away.paths).toContain("/unrestricted-svg");

      // At `self` again: the shell opened anew in the same context, the page frames the SVG it fetched while `unrestricted`.
      await patchAsOwner(d.base, artifact.id, { content_security_policy: "self" });
      f = await openArtifact(tab, d.base, artifact.id, 1);
      await f.evaluate(to => { (window as unknown as Probe).to = to; }, away.url);
      expect(await f.evaluate(() => (window as unknown as Probe).direct("self-direct"))).toBe("refused");
      expect(await f.evaluate(() => (window as unknown as Probe).through("self-svg"))).toBe("refused");
      expect(away.paths).toEqual(["/unrestricted-svg"]);
      await ctx.close();
    } finally {
      away.close();
    }
  }));
}

const SECRET = "e2e setup secret";

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

test("the Share dialog's Content Security Policy setting explains itself and sends nothing until the owner confirms", async ({ browser }) => {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const h = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(h.port !== port, `the daemon took port ${h.port}, not ${port}`);
  try {
    const { artifact } = await publish(h.base, h.token, "Site plan", { "index.html": "<title>Site plan</title><h1>Site plan</h1>" });
    const policy = async () => ((await api(h.base, h.token, `/api/artifacts/${artifact.id}`)) as { artifact: { content_security_policy: string } }).artifact.content_security_policy;
    const owner = await (await browser.newContext()).newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);

    await owner.goto(`${origin}/a/${artifact.id}`);
    await expect(owner.frameLocator("iframe.frame").locator("h1")).toHaveText("Site plan");
    await owner.getByRole("button", { name: "Share", exact: true }).click();
    const dialog = owner.getByRole("dialog", { name: "Share Site plan" });
    await expect(dialog).toBeVisible();
    const sw = dialog.getByRole("checkbox", { name: "Serve this clax without Clax's Content Security Policy" });
    await expect(sw).not.toBeChecked();
    await expect(dialog.getByTestId("csp-note")).toHaveCount(0);

    // On: the explanation first, and nothing sent.
    const patches: string[] = [];
    owner.on("request", r => { if (r.method() === "PATCH") patches.push(r.postData() ?? ""); });
    await sw.click();
    const ask = dialog.getByTestId("csp-confirm");
    await expect(ask).toContainText("Clax then sends no Content-Security-Policy holding its pages to their own site: they may load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.");
    await expect(ask.getByRole("button", { name: "Confirm" })).toBeFocused();
    await expect(sw).not.toBeChecked();
    expect(patches).toEqual([]);
    expect(await policy()).toBe("self");
    // Tab stays inside the dialog while it asks.
    await owner.keyboard.press("Tab");
    await expect(dialog.locator(":focus")).toHaveCount(1);

    // Cancel: still nothing sent, still `self`.
    await ask.getByRole("button", { name: "Cancel" }).click();
    await expect(ask).toHaveCount(0);
    await expect(sw).not.toBeChecked();
    expect(patches).toEqual([]);
    expect(await policy()).toBe("self");

    // Confirm: the daemon holds `unrestricted`, and the dialog says so.
    await sw.click();
    await ask.getByRole("button", { name: "Confirm" }).click();
    await expect.poll(policy).toBe("unrestricted");
    await expect(sw).toBeChecked();
    await expect(dialog.getByTestId("csp-note")).toHaveText("Served without Clax's Content Security Policy: its pages load and send where they choose.");
    expect(patches.map(p => JSON.parse(p))).toEqual([{ content_security_policy: "unrestricted" }]);

    // Off: `self` again at once, with no question.
    await sw.click();
    await expect.poll(policy).toBe("self");
    await expect(ask).toHaveCount(0);
    await expect(sw).not.toBeChecked();
    await expect(dialog.getByTestId("csp-note")).toHaveCount(0);
    expect(patches.map(p => JSON.parse(p))).toEqual([{ content_security_policy: "unrestricted" }, { content_security_policy: "self" }]);
  } finally {
    await h.stop();
  }
});

for (const engine of ENGINES) {
  test(`${engine.name()}: on a hosted Clax, a private unrestricted clax's service worker registers from a top-level visit of its site`, () => inEngine(engine, async browser => {
    const port = await freePort();
    const origin = `http://localhost:${port}`;
    const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
    const h = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
    test.skip(h.port !== port, `the daemon took port ${h.port}, not ${port}`);
    try {
      const { artifact } = await api(h.base, h.token, "/api/artifacts", {
        method: "POST",
        body: JSON.stringify({
          title: "Worker site",
          files: { "index.html": { content: "<title>Worker site</title><h1>Worker site</h1>", encoding: "utf8" }, "sw.js": { content: SW, encoding: "utf8" } },
        }),
      }) as { artifact: { id: string } };
      const changed = await api(h.base, h.token, `/api/artifacts/${artifact.id}`, {
        method: "PATCH", body: JSON.stringify({ content_security_policy: "unrestricted" }),
      }) as { artifact: { content_security_policy: string; site_access: string } };
      expect(changed.artifact).toMatchObject({ content_security_policy: "unrestricted", site_access: "private" });
      const ctx = await browser.newContext();
      const owner = await ctx.newPage();
      await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
      await owner.getByLabel("Your name").fill("Ada");
      await owner.getByRole("button", { name: "Create the first person" }).click();
      await owner.waitForURL(`${origin}/`);

      // From the shell's frame, the frame's partitioned site cookie does not go with the worker's script
      // fetch. Not run in WebKit: there an evaluate in a hosted shell's frame, which arrives through
      // `/_clax/session/start`, never returns, though the frame shows the page.
      if (engine !== webkit) {
        const framed = await openArtifact(owner, origin, artifact.id, 1);
        expect(await framed.evaluate(() => navigator.serviceWorker.register("sw.js").then(() => "registered", (e: Error) => `refused:${e.name}`))).toMatch(/^refused:/);
      }

      // Top-level, the site signs the owner in with a first-party cookie its worker's script fetch carries.
      const site = `http://${artifact.id}.localhost:${port}/`;
      await owner.goto(site);
      await owner.waitForURL(site);
      await expect(owner.locator("h1")).toHaveText("Worker site");
      const scriptURL = await owner.evaluate(async () => {
        await navigator.serviceWorker.register("sw.js");
        const reg = await navigator.serviceWorker.ready;
        return reg.active?.scriptURL ?? "";
      });
      expect(scriptURL).toBe(`${site}sw.js`);
      await owner.goto(`${site}hello.html`);
      await expect(owner.locator("body")).toHaveText("from the worker");
      await ctx.close();
    } finally {
      await h.stop();
    }
  }));
}
