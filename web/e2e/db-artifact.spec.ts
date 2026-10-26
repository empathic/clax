// One app reads another's documents with db.artifact(id): refused until the
// owner allows it, served after, followed while granted, and stopped by a
// revoke.
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
  // B's clax.json owns its declaration, so a new version changes it.
  if (JSON.stringify(caps) !== JSON.stringify({ db: {} })) await api(d.base, d.token, `/api/artifacts/${b}/versions`, { method: "POST", body: JSON.stringify({ if_version: 1, capabilities: caps, files: { "index.html": { content: SOURCE } } }) });
  return { a, b };
}

async function open(page: Page, a: string): Promise<Frame> {
  const f = await openArtifact(page, d.base, a, 1);
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
  // B's changes reach A when A's site is read again: no shell reports
  // them, so the bridge refetches on its polls and whenever the page comes
  // back into view, which the test brings about rather than waiting out a
  // poll (origin.ts UNWATCHED_POLL_MS).
  const backInView = () => f.evaluate(() => { document.dispatchEvent(new Event("visibilitychange")); });
  await setDoc(b, "runs/r2", { km: 7 });
  await backInView();
  await expect(f.locator("#runs")).toHaveText("5,7");

  // Revoked: the next refetch is refused, and the subscription ends with
  // `not_granted`; a change to B no longer reaches A.
  await api(d.base, d.token, `/api/artifacts/${a}/db-grants/${b}`, { method: "DELETE" });
  await setDoc(b, "runs/r3", { km: 9 });
  await backInView();
  await expect(f.locator("#err")).toHaveText("not_granted");
  await expect(f.locator("#runs")).toHaveText("5,7");
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
