// Tests in render (spec 2026-10-12-flutter-real-apps-design C2) in a real
// browser: a hidden draft holding `tests/*.test.js` loaded in the app's
// inspect view, whose `claxHost.runTests` has the bridge's harness
// (`bridge/src/harness.ts`) import the files as modules, drive the page with
// `ui`, write its documents, and answer `tests`, in both frame modes. The
// app's side is ./app-host.ts.
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

async function runTests(page: Page, id: string, mode: "subdomain" | "sandbox", call: string): Promise<Tests> {
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await page.goto(`${d.base}/a/${id}?app=1&inspect=1`);
  await contentFrame(page, id, 1);
  await initHost(page);
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, call);
  await expect.poll(async () => (await ofType(page, "tests")).length, { timeout: 30_000 }).toBeGreaterThan(0);
  return (await ofType(page, "tests"))[0] as Tests;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: runTests imports the test files and answers what passed`, async ({ page }) => {
    const id = await draft({ "index.html": PAGE, "tests/app.test.js": PASSING });
    const r = await runTests(page, id, mode, `runTests({ files: ["tests/app.test.js"] })`);
    expect(r).toEqual({ type: "tests", passed: 4, failed: [], skipped: 0 });
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
