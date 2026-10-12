// One app reads another's documents with db.artifact(id) (spec
// 2026-10-12-flutter-app-design §3.8): refused until the owner allows it,
// served after, live while granted, and stopped by a revoke.
import type { Frame, Page } from "@playwright/test";
import { test, expect, type Daemon, api, openArtifact, publishWith } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const SOURCE = `<!doctype html><html><head><title>Runs</title></head><body><p>runs</p></body></html>`;

/** App A: follows its own `marks` and, once asked, B's `runs`. */
const reader = (b: string) => `<!doctype html><html><head><title>Dashboard</title></head><body>
<pre id="runs">idle</pre><pre id="err"></pre><pre id="marks">0</pre>
<script>
(async () => {
  const db = await claude.use("db");
  db.collection("marks").onSnapshot(s => { document.getElementById("marks").textContent = String(s.size); });
  window.readOnce = () => db.artifact("${b}").doc("runs/r1").get().then(s => "km:" + s.data().km, e => e.code);
  window.follow = () => db.artifact("${b}").collection("runs").onSnapshot(
    s => { document.getElementById("runs").textContent = s.docs.map(x => x.data().km).join(","); },
    e => { document.getElementById("err").textContent = e.code; });
  window.ready = true;
})();
</script></body></html>`;

async function setDoc(aid: string, path: string, data: Record<string, unknown>) {
  await api(d.base, d.token, `/api/artifacts/${aid}/docs/${path}`, { method: "PUT", body: JSON.stringify({ data, lww: true }) });
}

/** Publishes B with one run, and A declaring the read of B. */
async function apps(tag: string, bCaps: (a: string) => Record<string, unknown> = () => ({ db: {} })) {
  const b = (await publishWith(d.base, d.token, `Runs ${tag}`, SOURCE, { db: {} })).artifact.id;
  await setDoc(b, "runs/r1", { km: 5 });
  const a = (await publishWith(d.base, d.token, `Dashboard ${tag}`, reader(b), { db: { read: [b] } })).artifact.id;
  const caps = bCaps(a);
  if (JSON.stringify(caps) !== JSON.stringify({ db: {} })) await api(d.base, d.token, `/api/artifacts/${b}`, { method: "PATCH", body: JSON.stringify({ capabilities: caps }) });
  return { a, b };
}

async function open(page: Page, a: string): Promise<Frame> {
  const f = await openArtifact(page, d.base, a, 1, "subdomain");
  await expect.poll(() => f.evaluate(() => (window as unknown as { ready?: boolean }).ready === true)).toBe(true);
  return f;
}

const readOnce = (f: Frame) => f.evaluate(() => (window as unknown as { readOnce(): Promise<string> }).readOnce());

test("a read is refused until the owner allows it, once per view", async ({ page }) => {
  const { a } = await apps("deny");
  const f = await open(page, a);
  const answer = readOnce(f);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Let “Dashboard deny” read “Runs deny”'s data?");
  await dialog.getByRole("button", { name: "Don't allow", exact: true }).click();
  expect(await answer).toBe("not_granted");
  // Not asked again in this view.
  expect(await readOnce(f)).toBe("not_granted");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const grants = await api(d.base, d.token, `/api/artifacts/${a}/db-grants`) as { grants: unknown[] };
  expect(grants.grants).toEqual([]);
});

test("Allow grants the read; the view follows the other app's changes until the grant is revoked", async ({ page }) => {
  const { a, b } = await apps("allow");
  const f = await open(page, a);
  const answer = readOnce(f);
  await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
  expect(await answer).toBe("km:5");
  const grants = await api(d.base, d.token, `/api/artifacts/${a}/db-grants`) as { grants: { source_id: string }[] };
  expect(grants.grants.map(g => g.source_id)).toEqual([b]);

  await f.evaluate(() => (window as unknown as { follow(): void }).follow());
  await expect(f.locator("#runs")).toHaveText("5");
  await setDoc(b, "runs/r2", { km: 7 });
  await expect(f.locator("#runs")).toHaveText("5,7");

  // Revoked: a change to B no longer reaches A. A's own change, made after
  // it, is the marker that the stream has delivered what it was going to.
  await api(d.base, d.token, `/api/artifacts/${a}/db-grants/${b}`, { method: "DELETE" });
  await setDoc(b, "runs/r3", { km: 9 });
  await setDoc(a, "marks/m1", { at: 1 });
  await expect(f.locator("#marks")).toHaveText("1");
  await expect(f.locator("#runs")).toHaveText("5,7");
  await expect(f.locator("#err")).toHaveText("");
  // The view asked once already: a read now is refused without a dialog.
  expect(await readOnce(f)).toBe("not_granted");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("a grant made in an earlier view, or a share, is read without asking", async ({ page }) => {
  const granted = await apps("earlier");
  await api(d.base, d.token, `/api/artifacts/${granted.a}/db-grants/${granted.b}`, { method: "PUT" });
  const f = await open(page, granted.a);
  expect(await readOnce(f)).toBe("km:5");

  const shared = await apps("share", a => ({ db: { share: { read: [a] } } }));
  const g = await open(page, shared.a);
  expect(await readOnce(g)).toBe("km:5");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("a viewer who is not the owner is never asked", async ({ page }) => {
  const { a } = await apps("lan");
  const f = await openArtifact(page, d.base, a, 1, "sandbox", { lan: true });
  await expect.poll(() => f.evaluate(() => (window as unknown as { ready?: boolean }).ready === true)).toBe(true);
  expect(await readOnce(f)).toBe("not_granted");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
