// Tests in render in a real
// browser: a hidden draft holding `tests/*.test.js` loaded in the app's
// inspect view, whose `claxHost.runTests` has the bridge's harness
// (`bridge/src/harness.ts`) import the files as modules, drive the page with
// `ui`, write its documents, and answer `tests`, in both frame modes; and
// `claxHost.runMigration` in the migration view (`&migrate=1`), where the
// page's own markup never runs and the harness imports an app bundle's
// `migrate.js` and calls it with the page's `db`. The app's side is
// ./app-host.ts.
import type { Page } from "@playwright/test";
import { host, hostChannel, initHost, ofType, type HostMessage } from "./app-host";
import { test, expect, type Daemon, api, contentFrame } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

// A counter that keeps its count in a document, with a second screen at #about.
const PAGE = `<!doctype html><html><head><title>Counter</title></head>
<body><h1 id="n">0</h1><button id="add">Add one</button><input id="name"><p id="hi"></p><section id="about" hidden>About this app</section>
<script type="module">
const db = await window.claude.use("db");
let n = 0;
const show = () => { document.getElementById("n").textContent = String(n); };
document.getElementById("add").onclick = async () => { n++; show(); await db.doc("counts/main").set({ n }); };
document.getElementById("name").oninput = e => { document.getElementById("hi").textContent = "Hi " + e.target.value; };
const route = () => { document.getElementById("about").hidden = location.hash !== "#about"; };
addEventListener("hashchange", route); route();
</script></body></html>`;

const PASSING = `test("adds one", async () => {
  await ui.click("Add one");
  await ui.waitFor(() => ui.text("#n") === "1");
  expect(ui.text("#n")).toBe("1");
});
test("greets", async () => {
  await ui.type("#name", "Ada");
  expect(ui.text("#hi")).toContain("Ada");
});
test("routes", async () => {
  await ui.go("about");
  await ui.waitFor(() => !document.getElementById("about").hidden);
  expect(ui.exists("#about")).toBeTruthy();
});
test("counts in the scratch documents", async () => {
  const db = await window.claude.use("db");
  await ui.waitFor(async () => (await db.doc("counts/main").get()).data()?.n === 1, 3000);
});`;

const FAILING = `test("wrong", () => { expect([1, 2]).toEqual([1, 3]); });
test("hangs", () => new Promise(() => {}));`;

type Tests = HostMessage & { passed?: number; failed?: { name: string; error: string }[]; skipped?: number; error?: string };

async function draft(files: Record<string, string>): Promise<string> {
  const r = await api(d.base, d.token, "/api/drafts", { method: "POST", body: JSON.stringify({ title: "Counter", capabilities: { db: {} }, files }) }) as { draft: { id: string } };
  return r.draft.id;
}

const MIGRATE = `export default async function migrate(db, { from, to }) {
  const d = db.doc("counts/main");
  const n = (await d.get()).data()?.n ?? 0;
  const booted = (await db.doc("boot/seen").get()).data() ?? null;
  await d.set({ count: n, migrated: [from, to], booted });
}`;

// A page that writes a document as it loads: the migration view never runs it.
const WRITES_ON_LOAD = `<!doctype html><html><head><title>Boot</title>
<script type="module">const db = await window.claude.use("db"); await db.doc("boot/seen").set({ at: 1 });</script>
<script>document.title = "ran";</script></head><body><button id="add">Add one</button></body></html>`;

// A migration that keeps writing until its document goes away.
const TICKS = `export default async function migrate(db) {
  for (let i = 1; ; i++) { await db.doc("ticks/t").set({ i }); await new Promise(r => setTimeout(r, 20)); }
}`;

const MIGRATE_VIEW = "?app=1&inspect=1&migrate=1";

// A viewer's own document and a shared one, before the update.
const SEED = `export default async function seed(db) {
  await db.doc("data/users/me/profile").set({ v: 1 });
  await db.doc("counts/main").set({ n: 1 });
}`;

// A migration that changes, adds and fails part way.
const BREAK = `export default async function migrate(db) {
  await db.doc("data/users/me/profile").set({ v: 2 });
  await db.doc("data/users/me/extra").set({ x: 1 });
  await db.doc("counts/main").set({ n: 2 });
  throw new Error("half way");
}`;

// What the viewer's documents hold, written where the token can read it.
const CHECK = `export default async function check(db) {
  const profile = (await db.doc("data/users/me/profile").get()).data() ?? null;
  const extra = (await db.doc("data/users/me/extra").get()).exists;
  const n = (await db.doc("counts/main").get()).data()?.n ?? null;
  await db.doc("check/out").set({ profile, extra, n });
}`;

/** Loads the migration view of `id` again, as the app does between runs. */
async function reopen(page: Page, id: string) {
  await page.goto(`${d.base}/a/${id}${MIGRATE_VIEW}`);
  await contentFrame(page, id, 1);
  await initHost(page);
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
}
const ticks = async (id: string) => ((await api(d.base, d.token, `/api/artifacts/${id}/docs/ticks/t`)) as { doc: { data: { i: number }; version: number } }).doc;
/** `ticks`, or null before the migration's first write. */
const ticksOrNull = (id: string) => ticks(id).catch((e: Error) => { if (e.message.includes(": 404 ")) return null; throw e; });

async function runTests(page: Page, id: string, mode: "subdomain" | "sandbox", call: string, answer = "tests", query = "?app=1&inspect=1"): Promise<Tests> {
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await page.goto(`${d.base}/a/${id}${query}`);
  await contentFrame(page, id, 1);
  await initHost(page);
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, call);
  await expect.poll(async () => (await ofType(page, answer)).length, { timeout: 30_000 }).toBeGreaterThan(0);
  return (await ofType(page, answer))[0] as Tests;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: runTests imports the test files and answers what passed`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "tests/app.test.js": PASSING });
    const r = await runTests(page, id, mode, `runTests({ files: ["tests/app.test.js"] })`);
    expect(r).toEqual({ type: "tests", passed: 4, failed: [], skipped: 0 });
  });

  test(`${mode}: runMigration imports migrate.js and runs it on the page's db, in a page whose own markup never runs`, async ({ page }) => {
    const id = await draft({ "index.html": WRITES_ON_LOAD, "migrate.js": MIGRATE });
    await api(d.base, d.token, `/api/artifacts/${id}/docs/counts/main`, { method: "PUT", body: JSON.stringify({ data: { n: 4 }, lww: true }) });
    const r = await runTests(page, id, mode, `runMigration({ file: "migrate.js", from: 2, to: 3 })`, "migration", MIGRATE_VIEW);
    expect(r).toEqual({ type: "migration", ok: true });
    const { doc } = await api(d.base, d.token, `/api/artifacts/${id}/docs/counts/main`) as { doc: { data: unknown } };
    expect(doc.data).toEqual({ count: 4, migrated: [2, 3], booted: null });
    const frame = await contentFrame(page, id, 1);
    expect(await frame.evaluate(() => [document.title, document.querySelectorAll("button").length, window.name])).toEqual(["", 0, "clax-migrate"]);
  });

  test(`${mode}: runMigration is refused in the inspect view`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "migrate.js": MIGRATE });
    const r = await runTests(page, id, mode, `runMigration({ file: "migrate.js", from: 1, to: 2 })`, "migration");
    expect(r).toEqual({ type: "migration", ok: false, error: "this view is not the migration view" });
  });

  test(`${mode}: a migration over its time is reported, and stops writing once its view is loaded again`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "migrate.js": TICKS });
    // The time covers loading migrate.js and the page's db too, so it is long
    // enough for both on a loaded machine; the migration never ends, so it
    // always runs over.
    const r = await runTests(page, id, mode, `runMigration({ file: "migrate.js", from: 1, to: 2, totalMs: 3000 })`, "migration", MIGRATE_VIEW);
    expect(r).toEqual({ type: "migration", ok: false, error: "it took longer than 3000 ms" });
    // Past its time it still writes: the bridge cannot stop it. Its first
    // write can land after the answer.
    await expect.poll(async () => (await ticksOrNull(id))?.version ?? 0).toBeGreaterThan(0);
    const late = (await ticks(id)).version;
    await expect.poll(async () => (await ticks(id)).version).toBeGreaterThan(late);
    // The app loads the view again, as WebViewInspector.runMigration does.
    await page.goto(`${d.base}/a/${id}${MIGRATE_VIEW}`);
    await contentFrame(page, id, 1);
    await initHost(page);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    const after = (await ticks(id)).version;
    // A run still going would write every 20 ms; none comes in ten times that.
    await page.waitForTimeout(200);
    expect((await ticks(id)).version).toBe(after);
  });

  test(`${mode}: a journaled migration that fails part way is put back from its journal, the viewer's data/users documents included`, async ({ page }) => {
    // This browser's viewer, whose private documents the migration writes:
    // the one the gallery's own lookup answers, made after its token request
    // has made the browser the owner. A lookup of the test's own could go
    // out before that, without a cookie, and mint a viewer of its own.
    const looked = page.waitForResponse(r => new URL(r.url()).pathname === "/api/viewers/me" && r.request().method() === "GET");
    await page.goto(`${d.base}/`);
    const me = ((await (await looked).json()) as { viewer: { public_id: string } }).viewer.public_id;
    const mine = (js: string) => js.replaceAll("data/users/me/", `data/users/${me}/`);
    const id = await draft({ "index.html": PAGE, "seed.js": mine(SEED), "break.js": mine(BREAK), "check.js": mine(CHECK) });
    expect(await runTests(page, id, mode, `runMigration({ file: "seed.js", from: 1, to: 2 })`, "migration", MIGRATE_VIEW)).toEqual({ type: "migration", ok: true });
    // The live run: every write waits for the app to keep its journal entry.
    await reopen(page, id);
    await host(page, `runMigration({ file: "break.js", from: 1, to: 2, journal: true })`);
    const entries: { path: string; before: unknown }[] = [];
    const acked = new Set<number>();
    await expect.poll(async () => {
      for (const j of (await ofType(page, "journal")) as unknown as { seq: number; path: string; before: unknown }[]) {
        if (acked.has(j.seq)) continue;
        acked.add(j.seq);
        entries.push({ path: j.path, before: j.before });
        await host(page, `journalAck({ seq: ${j.seq}, ok: true })`);
      }
      return (await ofType(page, "migration")).length;
    }, { timeout: 30_000 }).toBeGreaterThan(0);
    expect((await ofType(page, "migration"))[0]).toEqual({ type: "migration", ok: false, error: "the migration failed: half way" });
    expect(entries).toEqual([
      { path: `data/users/${me}/profile`, before: { v: 1 } },
      { path: `data/users/${me}/extra`, before: null },
      { path: "counts/main", before: { n: 1 } },
    ]);
    // The app loads the view again and replays the journal.
    await reopen(page, id);
    await host(page, `restoreJournal(${JSON.stringify({ entries })})`);
    await expect.poll(async () => ofType(page, "journalRestored"), { timeout: 30_000 }).toEqual([{ type: "journalRestored", ok: true }]);
    await reopen(page, id);
    await host(page, `runMigration({ file: "check.js", from: 1, to: 2 })`);
    await expect.poll(async () => ofType(page, "migration"), { timeout: 30_000 }).toEqual([{ type: "migration", ok: true }]);
    const { doc } = await api(d.base, d.token, `/api/artifacts/${id}/docs/check/out`) as { doc: { data: unknown } };
    expect(doc.data).toEqual({ profile: { v: 1 }, extra: false, n: 1 });
  });

  test(`${mode}: a migration that throws answers its error`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "migrate.js": `export default async () => { throw new TypeError("old shape"); }` });
    const r = await runTests(page, id, mode, `runMigration({ file: "migrate.js", from: 1, to: 2 })`, "migration", MIGRATE_VIEW);
    expect(r).toEqual({ type: "migration", ok: false, error: "the migration failed: TypeError: old shape" });
  });

  test(`${mode}: failures, a test over its limit and a file that does not load are reported`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "tests/bad.test.js": FAILING });
    const r = await runTests(page, id, mode, `runTests({ files: ["tests/bad.test.js", "tests/missing.test.js"], testMs: 500 })`);
    expect(r.passed).toBe(0);
    expect(r.skipped).toBe(0);
    expect(r.failed).toEqual([
      { name: "tests/missing.test.js", error: expect.stringContaining("the file did not load") },
      { name: "wrong", error: "Expected: [1,2] to equal [1,3]" },
      { name: "hangs", error: "it took longer than 500 ms" },
    ]);
  });
}
