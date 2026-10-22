import type { Page, Request } from "@playwright/test";
import { test, expect, type Daemon, api, openArtifact, publishWith } from "./fixtures";

// The page runtime on the artifact's own origin (docs/contract.md, `/_clax/api/`):
// the bridge reads and writes through its site, framed or top-level, and
// never through the public origin's `/api/`.

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const GUESTBOOK = `<!doctype html><html><head><title>Guestbook</title></head><body>
<input id="text"><button id="sign">Sign</button>
<pre id="me">waiting</pre><pre id="read"></pre><pre id="list">waiting</pre><pre id="err"></pre>
<script>
(async () => {
  const [db, user] = await Promise.all([claude.use("db"), claude.use("user")]);
  document.getElementById("me").textContent = JSON.stringify({ db: !!db, id: user && await user.id(), owner: user && await user.isOwner() });
  if (!db) return;
  const fail = e => { document.getElementById("err").textContent = e.code; };
  db.collection("data/guestbook/entries").orderBy("n").onSnapshot(s => {
    document.getElementById("list").textContent = s.docs.map(x => x.data().text).join(",");
  }, fail);
  document.getElementById("sign").onclick = async () => {
    const text = document.getElementById("text").value;
    try {
      await db.doc("data/guestbook/entries/" + text).set({ text, n: Date.now() });
      document.getElementById("read").textContent = (await db.doc("data/guestbook/entries/" + text).get()).data().text;
      // Nothing outside the rule that names the public level.
      await db.doc("tasks/t1").set({ x: 1 }).catch(fail);
    } catch (e) { fail(e); }
  };
})();
</script></body></html>`;

/** `artifact`'s site address on this daemon. */
const siteOf = (id: string) => `http://${id}.localhost:${d.port}`;

/** Every request `page` makes, as `METHOD URL`. */
function requests(page: Page): string[] {
  const seen: string[] = [];
  page.on("request", (r: Request) => { seen.push(`${r.method()} ${r.url()}`); });
  return seen;
}

test("a world-open site opened top-level writes and reads its db", async ({ browser }) => {
  const { artifact } = await publishWith(d.base, d.token, "World guestbook", GUESTBOOK, {
    db: { rules: [{ path: "data/guestbook", read: "public", write: "public" }] },
    user: {},
  });
  await api(d.base, d.token, `/api/artifacts/${artifact.id}`, { method: "PATCH", body: JSON.stringify({ site_access: "world" }) });
  const site = siteOf(artifact.id);

  // A stranger: a browser with no cookie of this daemon at all.
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const seen = requests(page);
  await page.goto(`${site}/`);
  await expect(page.locator("#me")).not.toHaveText("waiting");
  const me = JSON.parse((await page.locator("#me").textContent())!);
  expect(me).toMatchObject({ db: true, owner: false });
  expect(me.id).toMatch(/^v_[0-9a-f]{22}$/);
  await expect(page.locator("#list")).toHaveText("");
  await page.locator("#text").fill("ada");
  await page.locator("#sign").click();
  await expect(page.locator("#read")).toHaveText("ada");
  await expect(page.locator("#list")).toHaveText("ada");
  // The default rules grant a visitor nothing elsewhere.
  await expect(page.locator("#err")).toHaveText("invalid_argument");

  // Another stranger reads what the first wrote, under a visitor ID of their own.
  const other = await (await browser.newContext()).newPage();
  await other.goto(`${site}/`);
  await expect(other.locator("#list")).toHaveText("ada");
  await expect(other.locator("#me")).not.toHaveText("waiting");
  const them = JSON.parse((await other.locator("#me").textContent())!);
  expect(them.id).toMatch(/^v_/);
  expect(them.id).not.toBe(me.id);

  // Every runtime call went to the site's own /_clax/api/, none to the public origin.
  const runtime = seen.filter(r => r.includes("/_clax/api/"));
  expect(runtime.length).toBeGreaterThan(0);
  expect(runtime.every(r => r.split(" ")[1].startsWith(`${site}/_clax/api/`))).toBe(true);
  expect(seen.filter(r => r.includes("/api/artifacts/"))).toEqual([]);
  expect(seen.filter(r => r.split(" ")[1].startsWith(`${d.base}/`))).toEqual([]);
  await ctx.close();
  await other.context().close();
});

// The shell frames each site through a site code (POST
// /api/artifacts/<id>/site-code, /_clax/session/start); until the shell
// takes that path, a local shell's frame never arrives (qualifier record
// c0919532 on crates/clax-server/src/shell/boot.rs). Enable this with it.
test.fixme("a private site framed in the shell reads its db through its own origin", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Private guestbook", GUESTBOOK, { db: {}, user: {} });
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/docs/data/guestbook/entries/grace`, { method: "PUT", body: JSON.stringify({ data: { text: "grace", n: 1 } }) });
  const site = siteOf(artifact.id);
  const fromFrame: string[] = [];
  page.on("request", r => {
    const f = r.frame();
    if (f !== page.mainFrame() && f.url().startsWith(site)) fromFrame.push(`${r.method()} ${r.url()}`);
  });
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(frame.locator("#list")).toHaveText("grace");
  expect(JSON.parse((await frame.locator("#me").textContent())!)).toMatchObject({ db: true, owner: true });
  await frame.locator("#text").fill("hopper");
  await frame.locator("#sign").click();
  await expect(frame.locator("#read")).toHaveText("hopper");
  await expect(frame.locator("#list")).toHaveText("grace,hopper");
  // The frame's runtime calls are its site's; it never reaches the public origin's /api/.
  expect(fromFrame.some(r => r.includes(`${site}/_clax/api/docs`))).toBe(true);
  expect(fromFrame.filter(r => r.includes(`${d.base}/api/`) || r.includes("/api/artifacts/"))).toEqual([]);
});
