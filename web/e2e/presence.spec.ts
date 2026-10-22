import { test, expect, publishAs, registerSession } from "./fixtures";
import { hostedDaemon, memberOf, openAt, ownerOf, rightsOn, threadIn } from "./hosted-helpers";

// Two viewers: a local daemon's browsers are all its owner, so the second
// is a hosted daemon's other person.
test("two viewers see each other here, the location only in the panel, and away when hidden", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const s = await registerSession(d.base, d.token, "claude", "pres");
    const { artifact } = await publishAs(d.base, d.token, s.id, "Presence", { "index.html": "<main><h2>Quarterly goals</h2></main>" });
    const alex = await ownerOf(browser, origin, "alex");
    await openAt(alex, origin, artifact.id, 1);
    const tid = await threadIn(alex, artifact.id, "Two columns");
    const mia = await memberOf(browser, alex, origin, "Mia", rightsOn(artifact.id));
    await openAt(mia, origin, artifact.id, 1);
    if (!(await mia.locator("aside.sidebar").isVisible())) await mia.getByRole("button", { name: /Threads/ }).first().click();
    await mia.locator(`.thread-card[data-thread="${tid}"] .card-head`).click();
    // Mia replies, so she is a participant.
    const replied = await mia.evaluate(async ([id, t]) => (await fetch(`/api/artifacts/${id}/threads/${t}/comments`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body: "Agreed" }) })).status, [artifact.id, tid] as const);
    expect(replied).toBe(201);
    await alex.reload();
    await expect(alex.locator(".who .ppl .tok.here")).toHaveCount(2);
    await expect(alex.locator(".who")).not.toContainText("looking at");
    await alex.getByRole("button", { name: "People and agents" }).click();
    const panel = alex.getByRole("dialog", { name: "People and agents" });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("here, looking at");
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("In 1 thread.");
    await mia.evaluate(() => { Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true }); document.dispatchEvent(new Event("visibilitychange")); });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("away");
    await alex.context().close();
    await mia.context().close();
  } finally {
    await d.stop();
  }
});
