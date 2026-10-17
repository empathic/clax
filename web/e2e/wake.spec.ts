// A live agent that no comment wakes while it is idle is shown so on the
// page: its roster token is dashed and titled, and the people panel says
// "Won't be woken by your comments until it next acts." A follower of its
// session connecting (a notices long poll) refetches the agents and clears
// it. CLAX_WAKE_SHOTS=<dir> also photographs the panel at 1280 and 390, in
// light and dark.
import { test, expect, type Daemon, openArtifact, publishAs, registerSession } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

test("an agent no comment wakes is marked until a follower connects", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", `wake-${Math.random().toString(36).slice(2)}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Wake", { "index.html": PAGE });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const token = page.locator(".who .agt .tok.a");
  await expect(token).toHaveClass(/nowake/);
  await expect(token).toHaveAttribute("title", /won't be woken by comments until it next acts$/);
  await page.locator("button.who").click();
  const row = page.locator(".people .prow.agent");
  await expect(row.locator(".nowake")).toHaveText("Won't be woken by your comments until it next acts.");
  await page.locator(".people").getByRole("button", { name: "Close" }).click();
  // The view's topics are live, so the follower's connection reaches it as an event.
  await expect.poll(() => page.evaluate(() => (window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0)).toBeGreaterThan(0);
  const poll = new AbortController();
  void fetch(`${d.base}/api/sessions/${s.id}/notices?wait=30`, { headers: { authorization: `Bearer ${d.token}` }, signal: poll.signal }).catch(() => undefined);
  try {
    await expect(token).not.toHaveClass(/nowake/);
    await expect(token).not.toHaveAttribute("title", /won't be woken/);
    await page.locator("button.who").click();
    await expect(row).toHaveCount(1);
    await expect(row.locator(".nowake")).toHaveCount(0);
  } finally {
    poll.abort();
  }
});

const SHOTS = process.env.CLAX_WAKE_SHOTS ?? "";
for (const theme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test(`the marked agent, ${theme} at ${width}`, async ({ page }) => {
      test.skip(!SHOTS, "set CLAX_WAKE_SHOTS=<dir> to take the screenshots");
      const s = await registerSession(d.base, d.token, "claude", `wake-shot-${theme}-${width}`);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly goals", { "index.html": PAGE });
      await page.setViewportSize({ width, height: width > 700 ? 800 : 844 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await openArtifact(page, d.base, artifact.id, 1, "subdomain");
      await expect(page.locator(".who .agt .tok.a.nowake")).toHaveCount(1);
      await page.screenshot({ path: `${SHOTS}/${theme}-${width}-roster.png` });
      await page.locator("button.who").click();
      await expect(page.locator(".people .prow.agent .nowake")).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/${theme}-${width}-panel.png` });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    });
  }
}
