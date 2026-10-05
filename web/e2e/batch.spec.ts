import { test, expect, type Daemon, api, openArtifact, postThread, publishAs, registerSession } from "./fixtures";

// The send controls follow which agents are live on the daemon: each test
// has a daemon of its own, so no other test's sessions are among them.
let d: Daemon;
test.beforeEach(({ freshDaemon }) => { d = freshDaemon; });

async function fresh(title: string, n: number) {
  const s = await registerSession(d.base, d.token, "claude", `batch-${title}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": "<main><h2>Quarterly goals</h2></main>" });
  const ids: string[] = [];
  for (let i = 0; i < n; i++) ids.push((await postThread(d.base, artifact.id, `item ${i}`)).id);
  return { sid: s.id, aid: artifact.id, ids };
}
const panel = async (page: import("@playwright/test").Page) => {
  if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
};

for (const mode of ["subdomain", "sandbox"] as const) {
  // The shell and the daemon alone decide what the sidebar sends; the frame only shows the
  // page, the same in either mode: one mode each.
  if (mode === "subdomain") test(`${mode}: tick a shift range, send with a note, and the agent gets one delivery`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Batch ${mode}`, 4);
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const box = (id: string) => page.locator(`.thread-card[data-thread="${id}"] .thread-check`);
    await box(ids[0]).click();
    await box(ids[2]).click({ modifiers: ["Shift"] });
    const bar = page.getByRole("region", { name: "Selected comments" });
    await expect(bar.getByRole("status")).toHaveText("3 selected");
    await expect(bar).toContainText("sent together");
    // From the top of the list: the click on the third box may have scrolled
    // the sidebar, under which the bar stays put.
    await page.locator(".thread-card").first().evaluate(e => e.scrollIntoView({ block: "end" }));
    const barAboveCards = async () => (await bar.boundingBox())!.y - (await page.locator(".thread-card").first().boundingBox())!.y;
    await expect.poll(barAboveCards, { message: "the selection bar sits at the top of the sidebar" }).toBeLessThan(0);
    await expect(bar.getByRole("button", { name: "Send 3 to claude" })).toBeVisible();
    await expect(bar.getByRole("button", { name: "Choose the agent" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Send 4 unsent to claude" })).toBeVisible();
    await bar.getByLabel("Note for the agent (optional)").fill("Before the demo");
    await bar.getByLabel("Note for the agent (optional)").press("ControlOrMeta+Enter");
    await expect(bar).toHaveCount(0);
    const got = await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`);
    expect(got.feedback.map((f: { thread_id: string }) => f.thread_id)).toEqual(ids.slice(0, 3));
    expect(got.text.split("\n")[1]).toBe(`[clax] 3 comments on "Batch ${mode}", sent together by Viewer. Note: "Before the demo"`);
    await expect(page.locator(`.thread-card[data-thread="${ids[1]}"] .hist`)).toContainText("sent it with 2 others · “Before the demo”");
    await page.getByRole("button", { name: "Send 1 unsent to claude" }).click();
    await expect(page.getByRole("button", { name: /unsent to/ })).toHaveCount(0);
  });

  if (mode === "sandbox") test(`${mode}: with two live agents the caret picks one, and only that agent gets the rows`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Pick ${mode}`, 1);
    const other = await registerSession(d.base, d.token, "codex", `pick-${mode}`);
    await api(d.base, d.token, `/api/sessions/${other.id}/watches/${aid}`, { method: "PUT" });
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const card = page.locator(`.thread-card[data-thread="${ids[0]}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    await card.getByRole("menuitemradio", { name: "codex" }).click();
    // The click only starts the send; the rows exist once it is answered.
    const sent = page.waitForResponse(r => r.url().endsWith(`/threads/${ids[0]}/send`));
    await card.getByRole("button", { name: "Send to codex" }).click();
    expect((await sent).ok()).toBe(true);
    expect((await api(d.base, d.token, `/api/sessions/${other.id}/feedback?tier=piggyback`)).feedback).toHaveLength(1);
    expect((await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`)).feedback).toHaveLength(0);
    await page.reload();
    await panel(page);
    expect(await page.evaluate(id => localStorage.getItem(`clax.sendTo.${id}`), aid)).toMatch(/^a_/);
  });

  if (mode === "subdomain") test(`${mode}: with no live agent Send goes without to, and the comment waits for the next session`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Nobody ${mode}`, 1);
    await api(d.base, d.token, `/api/sessions/${sid}`, { method: "PATCH", body: JSON.stringify({ ended: true }) });
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const card = page.locator(`.thread-card[data-thread="${ids[0]}"]`);
    await expect(card.getByRole("button", { name: "Choose the agent" })).toHaveCount(0);
    const sent = page.waitForRequest(r => r.url().endsWith(`/threads/${ids[0]}/send`));
    await card.getByRole("button", { name: /^Send to / }).click();
    expect((await sent).postData() ?? "").not.toContain("\"to\"");
    const next = await registerSession(d.base, d.token, "codex", `nobody-next-${mode}`);
    await api(d.base, d.token, `/api/sessions/${next.id}/watches/${aid}`, { method: "PUT" });
    expect((await api(d.base, d.token, `/api/sessions/${next.id}/feedback?tier=piggyback`)).feedback).toHaveLength(1);
  });

  if (mode === "sandbox") test(`${mode}: a ticked thread that disappears leaves the selection, and the rest sends`, async ({ page }) => {
    const { aid, ids } = await fresh(`Prune ${mode}`, 2);
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    for (const id of ids) await page.locator(`.thread-card[data-thread="${id}"] .thread-check`).click();
    const bar = page.getByRole("region", { name: "Selected comments" });
    const count = bar.getByRole("status");
    await expect(count).toHaveText("2 selected");
    await page.request.post(`${d.base}/api/artifacts/${aid}/threads/${ids[0]}/resolve`, { headers: { origin: d.base } });
    await expect(count).toHaveText("1 selected");
    await bar.getByRole("button", { name: /^Send to / }).click();
    await expect(bar).toHaveCount(0);
  });
}
