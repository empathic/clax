// A clax that sets its own content policy (`content_policy: own`, PATCH
// /api/artifacts/<id>; docs/contract.md, "Security model"): while `locked`
// (every artifact's at first) its page's script from another origin and its
// WebSocket to that origin are refused by the policy and never reach it;
// once the owner lets it set its own policy, both work, and a service worker
// of the page's own registers and stays registered (no longer Clax's, which
// unregisters itself). Then the shell's switch, in a hosted Clax's Share
// dialog (web/shell/src/ui/ShareDialog.svelte): turning it on shows its
// explanation and sends nothing until the owner confirms, after which the
// daemon holds `own`; turning it off locks it again at once. Returning a
// clax to `locked` holds in a browser that kept its files from while it was
// `own`: a page framing one of its SVG files fetches nowhere else through it.
// On a hosted Clax, a private `own` clax's service worker registers from a
// top-level visit of its site.
import { createHash } from "node:crypto";
import { createServer as createHttpServer, type Server } from "node:http";
import { createServer, type AddressInfo } from "node:net";
import type { Frame } from "@playwright/test";
import { test, expect, type Daemon, NO_KEY_CONFIG, api, openArtifact, publish, startDaemon } from "./fixtures";
import { ownerCookie } from "./questions-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

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

const page = (away: { url: string; ws: string }) => `<!doctype html><html><head><title>Own policy</title>
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
</head><body><p>own</p></body></html>`;

/** A service worker of the page's own: it answers `hello.html` itself. */
const SW = `self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", e => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", e => { if (e.request.url.endsWith("/hello.html")) e.respondWith(new Response("from the worker", { headers: { "content-type": "text/plain" } })); });`;

type Win = { violations?: string[]; awayLoaded?: boolean; socket?: Promise<string> };
const violations = (f: Frame) => f.evaluate(() => (window as unknown as Win).violations ?? []);

/** Changes artifact `id` as the owner's browser does (`content_policy` and `site_access` each alone): with
 * the token alone, while an agent session of another test is live, `own` or `world` would wait for a
 * browser confirmation. */
async function patchAsOwner(base: string, id: string, change: { content_policy: "own" | "locked" } | { site_access: "world" | "private" }) {
  const res = await fetch(`${base}/api/artifacts/${id}`, {
    method: "PATCH", headers: { cookie: ownerCookie(base), origin: base, "content-type": "application/json" },
    body: JSON.stringify(change),
  });
  const text = await res.text();
  expect(res.status, text).toBe(200);
  expect((JSON.parse(text) as { artifact: Record<string, string> }).artifact).toMatchObject(change);
}

test("a locked clax's page loads no script from elsewhere and opens no WebSocket there; once it sets its own policy, both work and its own service worker stays", async ({ browser }) => {
  const away = await elsewhere();
  try {
    const { artifact } = await api(d.base, d.token, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({
        title: "Own policy",
        files: { "index.html": { content: page(away), encoding: "utf8" }, "sw.js": { content: SW, encoding: "utf8" } },
      }),
    }) as { artifact: { id: string; content_policy?: string } };
    expect(artifact.content_policy ?? "locked").toBe("locked");

    // Locked: both refused by the policy, and the other host never asked.
    const lockedCtx = await browser.newContext();
    const f = await openArtifact(await lockedCtx.newPage(), d.base, artifact.id, 1);
    // Refused as the socket opens, or as it is made (a browser may throw there).
    expect(await f.evaluate(() => (window as unknown as Win).socket)).toMatch(/^(refused|threw:)/);
    await expect.poll(async () => (await violations(f)).some(v => v.startsWith("script-src") && v.includes(away.url))).toBe(true);
    await expect.poll(async () => (await violations(f)).some(v => v.startsWith("connect-src ") && v.includes(away.ws))).toBe(true);
    expect(await f.evaluate(() => (window as unknown as Win).awayLoaded ?? false)).toBe(false);
    expect(away.hits()).toBe(0);
    await lockedCtx.close();

    // The owner lets it set its own policy; the daemon says so.
    await patchAsOwner(d.base, artifact.id, { content_policy: "own" });
    const got = await api(d.base, d.token, `/api/artifacts/${artifact.id}`) as { artifact: { content_policy: string } };
    expect(got.artifact.content_policy).toBe("own");

    // A fresh browser context, so no response of the locked page is reused.
    const ownCtx = await browser.newContext();
    const g = await openArtifact(await ownCtx.newPage(), d.base, artifact.id, 1);
    expect(await g.evaluate(() => (window as unknown as Win).socket)).toBe("echo:ping");
    await expect.poll(() => g.evaluate(() => (window as unknown as Win).awayLoaded ?? false)).toBe(true);
    expect(await violations(g)).toEqual([]);
    expect(away.hits()).toBeGreaterThanOrEqual(2);

    // Its own service worker registers, answers a frame of its scope, and stays. Registered from the
    // shell's frame, a private site answers the worker's script 401 `site_session_required`, so the site
    // is opened to the world first.
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
    await ownCtx.close();
  } finally {
    away.close();
  }
});

/** A server on another port of this host that answers every request 204 and keeps each path it is asked. */
async function listener(): Promise<{ url: string; paths: string[]; close: () => void }> {
  const paths: string[] = [];
  const s: Server = createHttpServer((req, res) => { paths.push(req.url ?? ""); res.writeHead(204).end(); });
  await new Promise<void>(r => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${(s.address() as AddressInfo).port}`, paths, close: () => { s.closeAllConnections(); s.close(); } };
}

/** A page that frames its version's `x.svg` (a document, served raw) and fetches `${to}/<label>` through the
 * framed SVG's window: "sent" when the policy let the request go, "refused" when it did not. */
const svgProbe = `<!doctype html><html><head><title>Relock</title></head><body><p>relock</p>
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

test("a clax returned to locked holds in a browser that kept its files from while it was own", async ({ browser }) => {
  const away = await listener();
  try {
    const { artifact } = await api(d.base, d.token, "/api/artifacts", {
      method: "POST",
      body: JSON.stringify({
        title: "Relock",
        files: { "index.html": { content: svgProbe, encoding: "utf8" }, "x.svg": { content: SVG, encoding: "utf8" } },
      }),
    }) as { artifact: { id: string } };
    await patchAsOwner(d.base, artifact.id, { content_policy: "own" });

    // One browser context throughout, so whatever it caches while `own` is what it holds after.
    const ctx = await browser.newContext();
    const tab = await ctx.newPage();
    let f = await openArtifact(tab, d.base, artifact.id, 1);
    await f.evaluate(to => { (window as unknown as Probe).to = to; }, away.url);
    expect(await f.evaluate(() => (window as unknown as Probe).through("own-svg"))).toBe("sent");
    await expect.poll(() => away.paths).toContain("/own-svg");

    // Locked again: the shell opened anew in the same context, the page frames the SVG it fetched while `own`.
    await patchAsOwner(d.base, artifact.id, { content_policy: "locked" });
    f = await openArtifact(tab, d.base, artifact.id, 1);
    await f.evaluate(to => { (window as unknown as Probe).to = to; }, away.url);
    expect(await f.evaluate(() => (window as unknown as Probe).direct("locked-direct"))).toBe("refused");
    expect(await f.evaluate(() => (window as unknown as Probe).through("locked-svg"))).toBe("refused");
    expect(away.paths).toEqual(["/own-svg"]);
    await ctx.close();
  } finally {
    away.close();
  }
});

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

test("the Share dialog's content policy switch explains itself and sends nothing until the owner confirms", async ({ browser }) => {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const h = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(h.port !== port, `the daemon took port ${h.port}, not ${port}`);
  try {
    const { artifact } = await publish(h.base, h.token, "Site plan", { "index.html": "<title>Site plan</title><h1>Site plan</h1>" });
    const policy = async () => ((await api(h.base, h.token, `/api/artifacts/${artifact.id}`)) as { artifact: { content_policy: string } }).artifact.content_policy;
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
    const sw = dialog.getByRole("checkbox", { name: "Let this clax set its own content policy" });
    await expect(sw).not.toBeChecked();
    await expect(dialog.getByTestId("own-note")).toHaveCount(0);

    // On: the explanation first, and nothing sent.
    const patches: string[] = [];
    owner.on("request", r => { if (r.method() === "PATCH") patches.push(r.postData() ?? ""); });
    await sw.click();
    const ask = dialog.getByTestId("own-confirm");
    await expect(ask).toContainText("Its pages may then load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.");
    await expect(ask.getByRole("button", { name: "Confirm" })).toBeFocused();
    await expect(sw).not.toBeChecked();
    expect(patches).toEqual([]);
    expect(await policy()).toBe("locked");
    // Tab stays inside the dialog while it asks.
    await owner.keyboard.press("Tab");
    await expect(dialog.locator(":focus")).toHaveCount(1);

    // Cancel: still nothing sent, still locked.
    await ask.getByRole("button", { name: "Cancel" }).click();
    await expect(ask).toHaveCount(0);
    await expect(sw).not.toBeChecked();
    expect(patches).toEqual([]);
    expect(await policy()).toBe("locked");

    // Confirm: the daemon holds `own`, and the dialog says so.
    await sw.click();
    await ask.getByRole("button", { name: "Confirm" }).click();
    await expect.poll(policy).toBe("own");
    await expect(sw).toBeChecked();
    await expect(dialog.getByTestId("own-note")).toHaveText("Sets its own content policy: its pages load and send where they choose.");
    expect(patches.map(p => JSON.parse(p))).toEqual([{ content_policy: "own" }]);

    // Off: locked again at once, with no question.
    await sw.click();
    await expect.poll(policy).toBe("locked");
    await expect(ask).toHaveCount(0);
    await expect(sw).not.toBeChecked();
    await expect(dialog.getByTestId("own-note")).toHaveCount(0);
    expect(patches.map(p => JSON.parse(p))).toEqual([{ content_policy: "own" }, { content_policy: "locked" }]);
  } finally {
    await h.stop();
  }
});

test("on a hosted Clax, a private own clax's service worker registers from a top-level visit of its site", async ({ browser }) => {
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
      method: "PATCH", body: JSON.stringify({ content_policy: "own" }),
    }) as { artifact: { content_policy: string; site_access: string } };
    expect(changed.artifact).toMatchObject({ content_policy: "own", site_access: "private" });
    const ctx = await browser.newContext();
    const owner = await ctx.newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);

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
});
