import { test, expect } from "@playwright/test";
import { openArtifact, postThread, publishAs, registerSession, setName, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: two viewers see each other here, the location only in the panel, and away when hidden`, async ({ browser }) => {
    const s = await registerSession(d.base, d.token, "claude", `pres-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Presence ${mode}`, { "index.html": "<main><h2>Quarterly goals</h2></main>" });
    const t = await postThread(d.base, artifact.id, "Two columns");
    const alex = await (await browser.newContext()).newPage();
    const mia = await (await browser.newContext()).newPage();
    await openArtifact(alex, d.base, artifact.id, 1, mode);
    await openArtifact(mia, d.base, artifact.id, 1, mode);
    await setName(alex, "alex");
    await setName(mia, "Mia");
    if (!(await mia.locator("aside.sidebar").isVisible())) await mia.getByRole("button", { name: /Threads/ }).first().click();
    await mia.locator(`.thread-card[data-thread="${t.id}"] .card-head`).click();
    // Mia replies, so she is a participant whose last viewed version (v1, public) Alex can read.
    await mia.request.post(`${d.base}/api/artifacts/${artifact.id}/threads/${t.id}/comments`, { headers: { origin: d.base }, data: { body: "Agreed" } });
    await alex.reload();
    await expect(alex.locator(".who .ppl .tok.here")).toHaveCount(2);
    await expect(alex.locator(".who")).not.toContainText("looking at");
    await alex.getByRole("button", { name: "People and agents" }).click();
    const panel = alex.getByRole("dialog", { name: "People and agents" });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("here, looking at");
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("Seen v1.");
    await mia.evaluate(() => { Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true }); document.dispatchEvent(new Event("visibilitychange")); });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("away");
    await alex.context().close();
    await mia.context().close();
  });
}
