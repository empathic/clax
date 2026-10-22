import { type Page } from "@playwright/test";
import { test, expect, lookedMarks, publishAs, publishNext, registerSession } from "./fixtures";
import { hostedDaemon, memberOf, openAt, ownerOf, rightsOn } from "./hosted-helpers";
const PAGE = "<main><h2>Quarterly goals</h2></main>";

async function comment(page: Page, aid: string, body: string, tid?: string): Promise<string> {
  return page.evaluate(async ([id, text, parent]) => {
    if (parent) return (await (await fetch(`/api/artifacts/${id}/threads/${parent}/comments`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body: text }) })).json()).thread.id;
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", text); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: f })).json()).thread.id;
  }, [aid, body, tid] as const);
}

// Two viewers: a local daemon's browsers are all its owner, so Mia is a
// hosted daemon's other person.
test("attention across viewers: addressed, new version, a mention, and looking clears", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const s = await registerSession(d.base, d.token, "claude", "att");
    const { artifact } = await publishAs(d.base, d.token, s.id, "Eyes", { "index.html": PAGE });
    const alex = await ownerOf(browser, origin, "alex");
    const mia = await memberOf(browser, alex, origin, "Mia", rightsOn(artifact.id));
    await openAt(alex, origin, artifact.id, 1);
    await openAt(mia, origin, artifact.id, 1);
    // Mia leaves before the reply that mentions her: a view left open shows
    // the new reply in its sidebar and marks it looked once it has been in
    // view a while, which a slow run reaches before Mia's gallery loads.
    await mia.goto(`${origin}/`);
    const tid = await comment(alex, artifact.id, "Two columns");
    await comment(alex, artifact.id, "@Mia which log?", tid);
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid] });
    await alex.goto(`${origin}/`);
    const card = alex.locator(".grp.needs .card-wrap", { hasText: "Eyes" });
    await expect(card.locator(".chip.you")).toHaveText("1 addressed in v2");
    await expect(card.locator(".chip.new")).toHaveText("v2 new");
    await expect(card.locator(".ft .seen")).toHaveText("seen v1");
    await mia.goto(`${origin}/`);
    await expect(mia.locator(".grp.needs .card-wrap", { hasText: "Eyes" }).locator(".chip.rep")).toHaveText("1 new reply");
    await openAt(alex, origin, artifact.id, 2);
    if (!(await alex.locator("aside.sidebar").isVisible())) await alex.getByRole("button", { name: /Threads/ }).first().click();
    await expect(alex.locator(`.thread-card[data-thread="${tid}"]`)).toBeVisible();
    await lookedMarks(alex);
    await alex.goto(`${origin}/`);
    await expect(alex.locator(".grp.needs .card-wrap", { hasText: "Eyes" })).toHaveCount(0);
    await expect(alex.locator(".grp.rest .card-wrap", { hasText: "Eyes" }).locator(".chip.oth")).toHaveText("1 open");
    await alex.context().close();
    await mia.context().close();
  } finally {
    await d.stop();
  }
});
