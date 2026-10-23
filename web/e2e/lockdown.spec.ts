// An artifact's page loads and fetches only its own site: its files, its
// assets, `data:` and `blob:` URLs (docs/contract.md, "Security model": the
// content policy): a fetch of another host or of the daemon's API and an
// image from another host are refused by the policy, while the bridge, the
// kit's modules, its own files (fetched too) and the `db` capability (on its
// own site) still work, and a comment clip inlines the page's own image.
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import { type Browser, type Frame, chromium, webkit } from "@playwright/test";
import { test, expect, type Daemon, api, colorStats, last, openArtifact, publish, record } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const KIT = new URL("../../apps/clax_app/assets/kit/", import.meta.url);
const kitFiles = Object.fromEntries(["kit.js", "preact.js", "icons.js", "kit.css"]
  .map(n => [`kit/${n}`, { content: readFileSync(new URL(n, KIT), "utf8"), encoding: "utf8" }]));

const DOT = `<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>`;

const page = (away: string) => `<!doctype html><html><head><title>Locked</title>
<script>
window.violations = [];
document.addEventListener("securitypolicyviolation", e => violations.push(e.effectiveDirective + " " + e.blockedURI));
</script>
<link rel="stylesheet" href="./kit/kit.css"></head><body>
<div id="bar"></div>
<img id="own" src="dot.svg" alt=""><img id="away" src="${away}/x.svg" alt="">
<script type="module">
import { html, render } from './kit/preact.js';
import { Button } from './kit/kit.js';
import { mark } from './own.js';
render(html\`<\${Button}>Kit \${mark}<//>\`, document.getElementById("bar"));
</script>
</body></html>`;

/** A server on another port of this host, counting the requests it gets. */
async function elsewhere(): Promise<{ url: string; hits: () => number; close: () => void }> {
  let hits = 0;
  const s: Server = createServer((_, res) => { hits++; res.writeHead(200, { "content-type": "image/svg+xml", "access-control-allow-origin": "*" }).end(DOT); });
  await new Promise<void>(r => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${(s.address() as AddressInfo).port}`, hits: () => hits, close: () => s.close() };
}

const violations = (f: Frame) => f.evaluate(() => (window as unknown as { violations?: string[] }).violations ?? []);

for (const mode of ["subdomain"] as const) {
  test(`${mode}: the page loads its own files and the kit, reaches db on its site, and cannot fetch or load from elsewhere`, async ({ page: p }) => {
    const away = await elsewhere();
    try {
      const { artifact } = await api(d.base, d.token, "/api/artifacts", {
        method: "POST",
        body: JSON.stringify({
          title: `Locked ${mode}`,
          capabilities: { db: {} },
          files: {
            "index.html": { content: page(away.url), encoding: "utf8" },
            "own.js": { content: "export const mark = 'own';", encoding: "utf8" },
            "dot.svg": { content: DOT, encoding: "utf8" },
            ...kitFiles,
          },
        }),
      }) as { artifact: { id: string } };
      const f = await openArtifact(p, d.base, artifact.id, 1);
      // The kit's modules and the page's own module ran.
      await expect(f.getByRole("button", { name: "Kit own" })).toBeVisible();
      // Its own image loaded; the other host's did not, and was never asked.
      await expect.poll(() => f.locator("#own").evaluate(i => (i as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
      await expect.poll(async () => (await violations(f)).some(v => v.startsWith("img-src ") && v.includes(away.url))).toBe(true);
      expect(away.hits()).toBe(0);
      // A fetch reaches the page's own files, and is refused by the policy
      // anywhere else: another host, the daemon's API and its other routes.
      const fetched = await f.evaluate(async ([a, daemon]) => {
        const tryFetch = (u: string) => fetch(u).then(r => `status:${r.status}`, e => `refused:${(e as Error).name}`);
        return [await tryFetch("own.js"), await tryFetch(`${a}/x.svg`), await tryFetch(`${daemon}/api/artifacts`), await tryFetch(`${daemon}/healthz`)];
      }, [away.url, d.base] as const);
      expect(fetched).toEqual(["status:200", "refused:TypeError", "refused:TypeError", "refused:TypeError"]);
      const v = await violations(f);
      expect(v.filter(x => x.startsWith("connect-src ")).length).toBeGreaterThanOrEqual(3);
      expect(away.hits()).toBe(0);
      // The bridge carries db to its own site.
      const got = await f.evaluate(async () => {
        const db = await (window as unknown as { claude: { use(n: string): Promise<any> } }).claude.use("db");
        await db.doc("notes/a").set({ t: "kept" });
        return (await db.doc("notes/a").get()).data().t as string;
      });
      expect(got).toBe("kept");
    } finally {
      away.close();
    }
  });
}

test("subdomain: a comment clip of the page's own image shows the image", async ({ page: p }) => {
  const { artifact } = await api(d.base, d.token, "/api/artifacts", {
    method: "POST",
    body: JSON.stringify({
      title: "Clip own image",
      files: {
        "index.html": { content: `<!doctype html><title>Pic</title><body style="margin:0;background:#fff"><img id="pic" src="dot.svg" width="120" height="120" alt="dot"></body>`, encoding: "utf8" },
        "dot.svg": { content: DOT, encoding: "utf8" },
      },
    }),
  }) as { artifact: { id: string } };
  await record(p);
  const f = await openArtifact(p, d.base, artifact.id, 1);
  await expect.poll(() => f.locator("#pic").evaluate(i => (i as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  await p.getByRole("button", { name: "Comment", exact: true }).click();
  await expect.poll(() => f.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  const fb = (await p.locator("iframe.frame").boundingBox())!;
  await p.mouse.click(fb.x + 60, fb.y + 60);
  const pick = await last(p, "clax:pick");
  expect(pick.clipError).toBeUndefined();
  expect((await colorStats(p, pick.pickId, [255, 0, 0], 30)).count).toBeGreaterThan(1000);
  expect(await violations(f)).toEqual([]);
});

// Two ways out that the policy on a page's own responses does not close by
// itself, in both engines: a same-origin frame of a response the content
// route did not answer (a 404, /healthz, the bridge), whose window's fetch
// would follow that response's policy; and a service worker of the page's
// own, answering a frame with a document that carries no policy. Every
// response for the artifact's content carries its policy, and a service
// worker script asked for there is Clax's, which unregisters itself.
const SW = `self.addEventListener("install", () => fetch("AWAY/sw-install").catch(() => {}));
self.addEventListener("fetch", e => { if (e.request.url.endsWith("/pwn.html")) e.respondWith(new Response('<script>fetch("AWAY/sw-doc").catch(() => {})</script>', { headers: { "content-type": "text/html" } })); });`;

for (const engine of [chromium, webkit]) {
  for (const mode of ["subdomain"] as const) {
    test(`${engine.name()} ${mode}: neither a framed 404, /healthz or bridge, nor a service worker of the page's own, reaches another host`, async () => {
      test.skip(!existsSync(engine.executablePath()), `${engine.name()} is not installed`);
      const away = await elsewhere();
      const browser: Browser = await engine.launch();
      try {
        const { artifact } = await api(d.base, d.token, "/api/artifacts", {
          method: "POST",
          body: JSON.stringify({
            title: `Escape ${engine.name()} ${mode}`,
            files: {
              "index.html": { content: "<!doctype html><title>Escape</title><p>escape</p>", encoding: "utf8" },
              "sw.js": { content: SW.replaceAll("AWAY", away.url), encoding: "utf8" },
            },
          }),
        }) as { artifact: { id: string } };
        const p = await browser.newPage();
        const f = await openArtifact(p, d.base, artifact.id, 1);
        // Borrowing a framed response's window.
        const borrowed = await f.evaluate(async a => {
          const out: Record<string, string> = {};
          for (const src of ["/healthz", "/nope", "missing.txt", "/_clax/bridge.js"]) {
            const fr = document.createElement("iframe");
            fr.src = src;
            await new Promise(r => { fr.onload = r; document.body.append(fr); });
            try {
              out[src] = await (fr.contentWindow as Window).fetch(`${a}/fr`).then(() => "reached", () => "refused");
            } catch {
              out[src] = "unreachable";
            }
          }
          return out;
        }, away.url);
        for (const [src, r] of Object.entries(borrowed)) expect(r, src).not.toBe("reached");
        // A service worker of the page's own.
        const registered = await f.evaluate(async () => {
          try {
            await navigator.serviceWorker.register("sw.js");
            return "registered";
          } catch (e) {
            return `refused:${(e as Error).name}`;
          }
        });
        if (registered === "registered") {
          // Clax's worker took its place and removes itself.
          await expect.poll(() => f.evaluate(async () => (await navigator.serviceWorker.getRegistrations()).length)).toBe(0);
        }
        const framed = await f.evaluate(async () => {
          const fr = document.createElement("iframe");
          fr.src = "pwn.html";
          await new Promise(r => { fr.onload = r; document.body.append(fr); });
          try { return (fr.contentDocument?.body?.textContent ?? "").slice(0, 40); } catch { return "unreachable"; }
        });
        expect(framed).not.toContain("<script>");
        expect(away.hits()).toBe(0);
      } finally {
        await browser.close();
        away.close();
      }
    });
  }
}

// A document the page makes from bytes of its own (a frame at a blob: or a
// data: URL of text/html, or a blob: script) keeps the page's policy, in
// both engines and frame modes: what its script fetches or loads from
// another host never arrives there, nor does a fetch the page makes through
// the blob frame's window. Each frame posts what its own attempts came to.
const FRAMED = (away: string, tag: string) => `<!doctype html><script>
const post = (k, v) => parent.postMessage({ framed: "${tag}", k, v }, "*");
post("ran", true);
fetch("${away}/${tag}-fetch").then(() => post("fetch", "reached"), () => post("fetch", "refused"));
const i = new Image(); i.onload = () => post("img", "loaded"); i.onerror = () => post("img", "refused"); i.src = "${away}/${tag}-img.svg";
</script>`;

for (const engine of [chromium, webkit]) {
  for (const mode of ["subdomain"] as const) {
    test(`${engine.name()} ${mode}: blob: and data: frames of text/html, and a blob: script, reach no other host`, async () => {
      test.skip(!existsSync(engine.executablePath()), `${engine.name()} is not installed`);
      const away = await elsewhere();
      const browser: Browser = await engine.launch();
      try {
        const { artifact } = await api(d.base, d.token, "/api/artifacts", {
          method: "POST",
          body: JSON.stringify({ title: `Framed ${engine.name()} ${mode}`, files: { "index.html": { content: "<!doctype html><title>Framed</title><p>framed</p>", encoding: "utf8" } } }),
        }) as { artifact: { id: string } };
        const p = await browser.newPage();
        const f = await openArtifact(p, d.base, artifact.id, 1);
        const out = await f.evaluate(async ({ blobDoc, dataDoc, awayUrl }) => {
          const got: Record<string, unknown> = {};
          (window as unknown as { framedGot: unknown }).framedGot = got;
          addEventListener("message", e => { const m = e.data as { framed?: string; k: string; v: unknown }; if (m?.framed) got[`${m.framed}.${m.k}`] = m.v; });
          const frame = (src: string) => new Promise<HTMLIFrameElement>(r => { const fr = document.createElement("iframe"); fr.onload = () => r(fr); fr.src = src; document.body.append(fr); });
          const blobFrame = await frame(URL.createObjectURL(new Blob([blobDoc], { type: "text/html" })));
          await frame(`data:text/html,${encodeURIComponent(dataDoc)}`);
          let borrowed: string;
          try {
            borrowed = await (blobFrame.contentWindow as Window).fetch(`${awayUrl}/borrowed`).then(() => "reached", () => "refused");
          } catch {
            borrowed = "unreachable";
          }
          const script = await new Promise<string>(r => {
            const s = document.createElement("script");
            s.onload = () => r("ran");
            s.onerror = () => r("refused");
            s.src = URL.createObjectURL(new Blob([`window.blobScript = fetch("${awayUrl}/script-fetch").then(() => "reached", () => "refused");`], { type: "text/javascript" }));
            document.head.append(s);
          });
          const scriptFetch = script === "ran" ? await (window as unknown as { blobScript: Promise<string> }).blobScript : "not run";
          return { borrowed, script, scriptFetch };
        }, { blobDoc: FRAMED(away.url, "blob"), dataDoc: FRAMED(away.url, "data"), awayUrl: away.url });
        expect(out.borrowed).not.toBe("reached");
        expect(out.scriptFetch).not.toBe("reached");
        // A frame whose script ran settles both its attempts, refused.
        const got = () => f.evaluate(() => (window as unknown as { framedGot: Record<string, unknown> }).framedGot);
        for (const tag of ["blob", "data"]) {
          await expect.poll(async () => { const g = await got(); return !g[`${tag}.ran`] || (`${tag}.fetch` in g && `${tag}.img` in g); }, { message: tag }).toBe(true);
          const g = await got();
          expect(g[`${tag}.fetch`], tag).not.toBe("reached");
          expect(g[`${tag}.img`], tag).not.toBe("loaded");
        }
        expect(away.hits()).toBe(0);
      } finally {
        await browser.close();
        away.close();
      }
    });
  }
}

/** Uploads an SVG asset of `fill` to artifact `id`; its URL as the daemon gives it. */
async function uploadAsset(id: string, fill: string): Promise<{ id: string; url: string }> {
  const form = new FormData();
  form.set("file", new Blob([DOT.replace("red", fill)], { type: "image/svg+xml" }), "dot.svg");
  const res = await fetch(`${d.base}/api/artifacts/${id}/assets`, { method: "POST", headers: { authorization: `Bearer ${d.token}` }, body: form });
  if (res.status !== 201) throw new Error(`${res.status} ${await res.text()}`);
  const body = await res.json() as { asset: { id: string }; url: string };
  return { id: body.asset.id, url: body.url };
}

/** The site origin of artifact `id` on the daemon at `d.base`. */
const siteOf = (id: string) => `http://${id}.localhost:${new URL(d.base).port}`;

/** Marks artifact `id` as made before sites (`artifacts.legacy_paths`, as the
 * migration that brought sites sets it on every artifact it finds). */
async function makeLegacy(id: string): Promise<void> {
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(join(d.home, "clax.db"));
  try {
    db.exec("PRAGMA busy_timeout = 5000");
    db.prepare("UPDATE artifacts SET legacy_paths = 1 WHERE id = ?").run(id);
  } finally { db.close(); }
}

const loadedIn = (f: Frame) => (k: string) => f.locator(`#${k}`).evaluate(i => (i as HTMLImageElement).complete ? (i as HTMLImageElement).naturalWidth > 0 : null);

test("a page loads its own site's assets and not another app's", async ({ page: p }) => {
  const mk = async (title: string) => (await publish(d.base, d.token, title, { "index.html": "<p>" })).artifact.id as string;
  const [a, b] = [await mk("Mine"), await mk("Theirs")];
  const mine = await uploadAsset(a, "green");
  const theirs = await uploadAsset(b, "blue");
  expect(mine.url).toBe(`/_clax/blob/${mine.id}`);
  const imgs = {
    mineRel: mine.url,
    // The address a local daemon gave pages for their own assets before
    // each artifact had a site, which pages and their documents keep.
    mineAbs: `${siteOf(a)}/_blob/${a}/${mine.id}`,
    mineScoped: `/_blob/${a}/${mine.id}`,
    theirsRel: `/_clax/blob/${theirs.id}`,
    theirsAbs: `${d.base}/_clax/blob/${theirs.id}`,
    theirsUnderMine: `${d.base}/_blob/${a}/${theirs.id}`,
    theirsScoped: `/_blob/${b}/${theirs.id}`,
    theirsOld: `${d.base}/_blob/${theirs.id}`,
  };
  const html = `<!doctype html><title>Assets</title>${Object.entries(imgs).map(([k, u]) => `<img id="${k}" src="${u}" alt="">`).join("")}`
    + `<img id="viaHelper" alt=""><script>document.getElementById("viaHelper").src = claude.assetUrl(${JSON.stringify(mine.id)});</script>`;
  await publish(d.base, d.token, "Mine", { "index.html": html }, 1, a);
  const f = await openArtifact(p, d.base, a, 2);
  const loaded = loadedIn(f);
  // Its own asset, by the URL the assets API returns, by
  // `claude.assetUrl(id)` (its site's address) and by the earlier address.
  for (const k of ["mineRel", "viaHelper", "mineAbs", "mineScoped"]) await expect.poll(() => loaded(k), { message: k }).toBe(true);
  const origin = await f.evaluate(() => location.origin);
  expect(origin).toBe(siteOf(a));
  expect(await f.locator("#viaHelper").getAttribute("src")).toBe(`${origin}${mine.url}`);
  // Another app's: not on this site, and refused off it by the policy.
  for (const k of ["theirsRel", "theirsAbs", "theirsUnderMine", "theirsScoped", "theirsOld"]) await expect.poll(() => loaded(k), { message: k }).toBe(false);
  // `"/_blob/" + id` names a file of this artifact's, made after sites: none.
  const old = await f.evaluate(id => new Promise<boolean>(r => { const i = new Image(); i.onload = () => r(true); i.onerror = () => r(false); i.src = "/_blob/" + id; }), mine.id);
  expect(old).toBe(false);
  // Fetching: its own asset, never another's.
  const got = await f.evaluate(async u => Promise.all(u.map(x => fetch(x).then(r => `status:${r.status}`, () => "refused"))),
    [imgs.mineRel, imgs.mineAbs, imgs.mineScoped, imgs.theirsAbs, imgs.theirsRel, imgs.theirsScoped]);
  expect(got).toEqual(["status:200", "status:200", "status:200", "refused", "status:404", "status:404"]);
});

test("a legacy artifact's page loads its assets at the addresses it was given, and not another app's", async ({ page: p }) => {
  const mk = async (title: string) => (await publish(d.base, d.token, title, { "index.html": "<p>" })).artifact.id as string;
  const [a, b] = [await mk("Legacy mine"), await mk("Legacy theirs")];
  await makeLegacy(a);
  await makeLegacy(b);
  const mine = await uploadAsset(a, "green");
  const theirs = await uploadAsset(b, "blue");
  // A legacy artifact's uploads are named as its pages name them.
  expect(mine.url).toBe(`/_blob/${mine.id}`);
  const imgs = {
    mineRel: mine.url,
    mineAbs: `${siteOf(a)}/_blob/${a}/${mine.id}`,
    mineScoped: `/_blob/${a}/${mine.id}`,
    mineSite: `/_clax/blob/${mine.id}`,
    theirsRel: `/_blob/${theirs.id}`,
    theirsScoped: `/_blob/${b}/${theirs.id}`,
    theirsUnderMine: `/_blob/${a}/${theirs.id}`,
    theirsAbs: `${siteOf(b)}/_blob/${theirs.id}`,
  };
  const html = `<!doctype html><title>Legacy assets</title>${Object.entries(imgs).map(([k, u]) => `<img id="${k}" src="${u}" alt="">`).join("")}`
    + `<img id="viaHelper" alt=""><script>document.getElementById("viaHelper").src = claude.assetUrl(${JSON.stringify(mine.id)});</script>`;
  await publish(d.base, d.token, "Legacy mine", { "index.html": html }, 1, a);
  const f = await openArtifact(p, d.base, a, 2);
  const loaded = loadedIn(f);
  for (const k of ["mineRel", "mineAbs", "mineScoped", "mineSite", "viaHelper"]) await expect.poll(() => loaded(k), { message: k }).toBe(true);
  for (const k of ["theirsRel", "theirsScoped", "theirsUnderMine", "theirsAbs"]) await expect.poll(() => loaded(k), { message: k }).toBe(false);
  const got = await f.evaluate(async u => Promise.all(u.map(x => fetch(x).then(r => `status:${r.status}`, () => "refused"))),
    [imgs.mineRel, imgs.mineScoped, imgs.theirsRel, imgs.theirsScoped, imgs.theirsAbs]);
  expect(got).toEqual(["status:200", "status:200", "status:404", "status:404", "refused"]);
});
