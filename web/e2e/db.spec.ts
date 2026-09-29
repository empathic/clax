import { test, expect } from "@playwright/test";
import { api, openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const BOARD = `<!doctype html><html><head><title>Board</title></head><body>
<button id="add">Add</button><button id="lock">Lock</button>
<pre id="out">waiting</pre><pre id="lockout"></pre><pre id="err"></pre><pre id="grammar"></pre>
<script>
(async () => {
  const db = await claude.use("db");
  const out = document.getElementById("out");
  db.collection("cards").orderBy("at").onSnapshot(s => {
    const last = s.docs.at(-1);
    out.textContent = JSON.stringify({ n: s.size, lag: last ? Date.now() - last.data().at : null, changes: s.docChanges().map(c => c.type) });
  }, e => { document.getElementById("err").textContent = "snapshot:" + e.code; });
  document.getElementById("add").onclick = () => db.collection("cards").add({ at: Date.now() })
    .catch(e => { document.getElementById("err").textContent = e.code; });
  document.getElementById("lock").onclick = async () => {
    const r = await db.doc("locks/editor").acquire({ holder: String(Math.random()), ttlMs: 60000 });
    document.getElementById("lockout").textContent = String(r.acquired);
  };
  try { db.doc("cards"); } catch (e) { document.getElementById("grammar").textContent = e instanceof TypeError ? "TypeError" : "other"; }
})();
</script></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a write in one tab reaches the other tab's onSnapshot through the event stream`, async ({ browser }) => {
    const { artifact } = await publishWith(d.base, d.token, `Board ${mode}`, BOARD, { db: {} });
    const ctx = await browser.newContext();
    const [pa, pb] = [await ctx.newPage(), await ctx.newPage()];
    const a = await openArtifact(pa, d.base, artifact.id, 1, mode);
    const b = await openArtifact(pb, d.base, artifact.id, 1, mode);
    await expect(a.locator("#out")).toHaveText(JSON.stringify({ n: 0, lag: null, changes: [] }));
    await expect(b.locator("#out")).toHaveText(JSON.stringify({ n: 0, lag: null, changes: [] }));
    await expect(a.locator("#grammar")).toHaveText("TypeError");
    await a.locator("#add").click();
    await expect(b.locator("#out")).toContainText('"n":1');
    const seen = JSON.parse((await b.locator("#out").textContent())!);
    expect(seen.changes).toEqual(["added"]);
    // Pushed, not polled: well inside the first stream retry (RETRY_MS[0], 5 s) and
    // the outage poll (POLL_MS, 30 s), with room for a loaded machine.
    expect(seen.lag).toBeLessThan(4_000);
    await a.locator("#lock").click();
    await expect(a.locator("#lockout")).toHaveText("true");
    await b.locator("#lock").click();
    await expect(b.locator("#lockout")).toHaveText("false");
    await ctx.close();
  });
}

test("LAN: an unnamed viewer reads but cannot write; naming them makes them a writer", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Board LAN", BOARD, { db: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(f.locator("#out")).toHaveText(JSON.stringify({ n: 0, lag: null, changes: [] }));
  await f.locator("#add").click();
  await expect(f.locator("#err")).toHaveText("invalid_argument");
  const name = page.getByRole("textbox", { name: "Your name" });
  await name.fill("Sam");
  await Promise.all([
    page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"),
    name.press("Enter"),
  ]);
  await f.locator("#add").click();
  await expect(f.locator("#out")).toContainText('"n":1');
});

const PROBE = `<!doctype html><html><head><title>Probe</title></head><body><p id="p">probe</p></body></html>`;

/** Runs `fn(db, arg)` in the frame's page with its `db` namespace. */
type Db = unknown;
async function withDb<T>(f: import("@playwright/test").Frame, fn: string, arg: unknown = null): Promise<T> {
  return f.evaluate(async ([body, a]) => {
    const db = await (window as unknown as { claude: { use(n: string): Promise<Db> } }).claude.use("db");
    return new Function("db", "arg", `return (async () => { ${body} })()`)(db, a);
  }, [fn, arg] as const);
}

async function publicId(page: import("@playwright/test").Page): Promise<string> {
  return page.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).viewer.public_id as string);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: get, update and delete round trip, and a where + limit query`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Probe ${mode}`, PROBE, { db: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    const r = await withDb<unknown>(f, `
      const t = db.doc("tasks/t1");
      const out = [];
      out.push((await t.get()).exists);
      await t.set({ title: "Ship", n: 1 });
      out.push((await t.get()).data());
      await t.update({ n: 2, done: true });
      out.push((await t.get()).data());
      await t.delete();
      const gone = await t.get();
      out.push([gone.exists, gone.data() === undefined]);
      for (let i = 1; i <= 6; i++) await db.doc("tasks/q" + i).set({ n: i, kind: i % 2 ? "odd" : "even" });
      const q = await db.collection("tasks").where("kind", "==", "odd").where("n", ">", 1).orderBy("n", "desc").limit(1).get();
      out.push(q.docs.map(d => [d.id, d.data().n]));
      const all = await db.collection("tasks").where("n", "in", [2, 4]).get();
      out.push(all.docs.map(d => d.id).sort());
      try { await db.collection("tasks").where("n", "~", 1).get(); } catch (e) { out.push(e.code); }
      return out;`);
    expect(r).toEqual([false, { title: "Ship", n: 1 }, { title: "Ship", n: 2, done: true }, [false, true], [["q5", 5]], ["q2", "q4"], "invalid_argument"]);
  });

  test(`${mode}: a viewer's data/users/<id>/ documents are invisible to another viewer`, async ({ browser }) => {
    const { artifact } = await publishWith(d.base, d.token, `Private ${mode}`, PROBE, { db: {} });
    const [ca, cb] = [await browser.newContext(), await browser.newContext()];
    const [pa, pb] = [await ca.newPage(), await cb.newPage()];
    const fa = await openArtifact(pa, d.base, artifact.id, 1, mode);
    const fb = await openArtifact(pb, d.base, artifact.id, 1, mode);
    const [ida, idb] = [await publicId(pa), await publicId(pb)];
    expect(ida).not.toBe(idb);
    await withDb(fa, `await db.doc("data/users/" + arg + "/profile").set({ pick: 3 });`, ida);
    expect(await withDb(fa, `return (await db.doc("data/users/" + arg + "/profile").get()).data();`, ida)).toEqual({ pick: 3 });
    const seenByB = await withDb<unknown>(fb, `
      const out = [];
      out.push((await db.doc("data/users/" + arg.a + "/profile").get()).exists);
      out.push((await db.collection("data/users/" + arg.a).get()).size);
      try { await db.doc("data/users/" + arg.a + "/profile").set({ pick: 9 }); out.push("wrote"); } catch (e) { out.push(e.code); }
      await db.doc("data/users/" + arg.b + "/profile").set({ pick: 5 });
      out.push((await db.doc("data/users/" + arg.b + "/profile").get()).data());
      return out;`, { a: ida, b: idb });
    expect(seenByB).toEqual([false, 0, "invalid_argument", { pick: 5 }]);
    expect(await withDb(fa, `return (await db.doc("data/users/" + arg + "/profile").get()).data();`, ida)).toEqual({ pick: 3 });
    await ca.close();
    await cb.close();
  });

  test(`${mode}: use("db") resolves null for a page that does not declare it`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Undeclared ${mode}`, PROBE, {});
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    expect(await f.evaluate(async () => (await (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("db")) === null)).toBe(true);
  });
}

test("LAN: naming the viewer refetches subscriptions under its new level", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Board LAN read", BOARD, { db: { rules: [{ path: "", read: "interact" }] } });
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/docs/cards/c1`, { method: "PUT", body: JSON.stringify({ data: { at: 0 }, lww: true }) });
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(f.locator("#out")).toHaveText(JSON.stringify({ n: 0, lag: null, changes: [] }));
  await page.getByRole("textbox", { name: "Your name" }).fill("Ada");
  await page.getByRole("textbox", { name: "Your name" }).press("Enter");
  await expect(f.locator("#out")).toContainText('"n":1');
});
