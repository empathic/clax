import { test, expect } from "@playwright/test";
import { openArtifact, publishWith, startDaemon } from "./fixtures";

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
  test(`${mode}: a write in one tab reaches the other tab's onSnapshot within 500 ms`, async ({ browser }) => {
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
    expect(seen.lag).toBeLessThan(500);
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
