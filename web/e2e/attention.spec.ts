import { type Page } from "@playwright/test";
import { test, expect, type Daemon, lookedMarks, openArtifact, publishAs, publishNext, registerSession } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

async function name(page: Page, n: string) {
  await page.evaluate(who => fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: who }) }), n);
}
async function comment(page: Page, aid: string, body: string, tid?: string): Promise<string> {
  return page.evaluate(async ([id, text, parent]) => {
    if (parent) return (await (await fetch(`/api/artifacts/${id}/threads/${parent}/comments`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body: text }) })).json()).thread.id;
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", text); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: f })).json()).thread.id;
  }, [aid, body, tid] as const);
}

// The shell and the daemon alone decide what this shows: the frame only
// shows the page, the same in either frame mode, so it runs in one mode.
for (const mode of ["subdomain"] as const) {
  test(`${mode}: attention across viewers: addressed, new version, a mention, and looking clears`, async ({ browser }) => {
    const s = await registerSession(d.base, d.token, "claude", `att-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Eyes ${mode}`, { "index.html": PAGE });
    const alex = await (await browser.newContext()).newPage();
    const mia = await (await browser.newContext()).newPage();
    await openArtifact(alex, d.base, artifact.id, 1, mode);
    // Mia is on another machine: the owner's browsers are all one viewer.
    await openArtifact(mia, d.base, artifact.id, 1, mode, { lan: true });
    await name(alex, "alex");
    await name(mia, "Mia");
    const tid = await comment(alex, artifact.id, "Two columns");
    await comment(alex, artifact.id, "@Mia which log?", tid);
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid] });
    await alex.goto(`${d.base}/`);
    const card = alex.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` });
    await expect(card.locator(".chip.you")).toHaveText("1 addressed in v2");
    await expect(card.locator(".chip.new")).toHaveText("v2 new");
    await expect(card.locator(".ft .seen")).toHaveText("seen v1");
    await mia.goto(`${d.base}/`);
    await expect(mia.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` }).locator(".chip.rep")).toHaveText("1 new reply");
    await openArtifact(alex, d.base, artifact.id, 2, mode);
    if (!(await alex.locator("aside.sidebar").isVisible())) await alex.getByRole("button", { name: /Threads/ }).first().click();
    await expect(alex.locator(`.thread-card[data-thread="${tid}"]`)).toBeVisible();
    await lookedMarks(alex);
    await alex.goto(`${d.base}/`);
    await expect(alex.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` })).toHaveCount(0);
    await expect(alex.locator(".grp.rest .card-wrap", { hasText: `Eyes ${mode}` }).locator(".chip.oth")).toHaveText("1 open");
    await alex.context().close();
    await mia.context().close();
  });
}
