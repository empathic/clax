// An artifact's page loads and fetches only its own files, its assets,
// `data:` and `blob:` URLs (docs/contract.md, "Security model": the content
// policy): in both frame modes a fetch of another host or of the daemon's
// API and an image from another host are refused by the policy, while the
// bridge, the kit's modules, its own files (fetched too) and the `db`
// capability (through the shell) still work, and a comment clip inlines the
// page's own image.
import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import type { Frame } from "@playwright/test";
import { test, expect, type Daemon, api, colorStats, last, openArtifact, record } from "./fixtures";

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

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the page loads its own files and the kit, reaches db through the shell, and cannot fetch or load from elsewhere`, async ({ page: p }) => {
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
      const f = await openArtifact(p, d.base, artifact.id, 1, mode);
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
      // The bridge carries db through the shell.
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

// In sandbox mode every image is cross-origin to the page's opaque origin,
// so its clips carry placeholders for images (web/bridge/src/clip.ts).
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
  const f = await openArtifact(p, d.base, artifact.id, 1, "subdomain");
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
