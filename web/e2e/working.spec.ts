import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { api, openArtifact, postThread, publishAs, publishWith, reach, registerSession, setWorking, skewWorking, startDaemon, setName } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the page reads working state through the comments capability`, async ({ page }) => {
    const html = readFileSync(new URL("./pages/working-cap.html", import.meta.url), "utf8");
    const s = await registerSession(d.base, d.token, "codex", `cap-${mode}`);
    const { artifact } = await publishWith(d.base, d.token, `Cap ${mode}`, html, { comments: {} });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#state")).toHaveText("none");
    await setName(page, "Alex");
    // A write as the viewer needs their click in the page 5.5 s clear of
    // their input to the shell (the name field).
    await page.waitForTimeout(5_700);
    await reach(page, frame.locator("#add"));
    await frame.locator("#add").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect.poll(() => frame.locator("body").getAttribute("data-handle")).toBeTruthy();
    const tid = (await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`)).threads[0].id as string;
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Chart", thread_ids: [tid] });
    await expect(frame.locator("#state")).toHaveText("Codex|Chart|1|0");
    expect(await frame.locator("body").getAttribute("data-handle")).not.toBe(tid);
    await api(d.base, d.token, `/api/sessions/${s.id}/working/${artifact.id}`, { method: "DELETE" });
    await expect(frame.locator("#state")).toHaveText("none");
  });

  test(`${mode}: the summary, roster, marker, pin and gallery chip follow the working record`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `work-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Working ${mode}`, { "index.html": PAGE });
    const t = await postThread(d.base, artifact.id, "@agent two columns");
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const line1 = page.locator(".who .sum b.l1");
    await expect(line1).toHaveText("Nobody working");
    await expect(page.locator(".who [role=status]")).toHaveAttribute("aria-live", "polite");
    await api(d.base, d.token, `/api/sessions/${s.id}/feedback?tier=piggyback`);
    await expect(line1).toHaveText("claude working on 1");
    await expect(page.locator(".who .agt .tok.work")).toHaveCount(1);
    await expect(page.locator(".topbar")).toHaveClass(/working/);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.thread-card[data-thread="${t.id}"]`);
    await expect(card.locator(".st.ag")).toContainText("claude is working on it");
    await expect(card.locator(".hist")).toContainText("claude working on it");
    await expect(page.locator(".thread-pin.onit")).toHaveCount(1);
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Two columns" });
    await expect(line1).toHaveText("claude: Two columns");
    const gallery = await page.context().newPage();
    await gallery.goto(`${d.base}/`);
    await expect(gallery.locator(".card-wrap", { hasText: `Working ${mode}` }).locator(".chip.ag")).toHaveText("claude working on 1");
    await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${t.id}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Done.", author_kind: "agent" }) });
    await expect(card.locator(".st.ag")).toHaveCount(0);
    await expect(line1).toHaveText("Nobody working");
    await expect(gallery.locator(".card-wrap", { hasText: `Working ${mode}` }).locator(".chip.ag")).toHaveCount(0, { timeout: 10_000 });
  });

  test(`${mode}: a record lapses 2 minutes after its last renewal`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "pi", `lapse-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Lapse ${mode}`, { "index.html": PAGE });
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Tidying" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator(".who .sum b.l1")).toHaveText("pi: Tidying");
    await skewWorking(d.base, d.token, 121);
    await expect(page.locator(".who .sum b.l1")).toHaveText("Nobody working");
  });
}

test("at phone width in dark mode with reduced motion the roster shrinks and nothing moves", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "phone-work");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title for a phone-width working check", { "index.html": PAGE });
  await setWorking(d.base, d.token, s.id, artifact.id, { message: "Rebuilding the quarterly chart with the new numbers" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator(".who .agt .tok.work")).toBeVisible();
  await expect(page.locator(".who .sum")).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await page.locator(".who .tok.work").evaluate(e => getComputedStyle(e, "::before").animationName)).toBe("none");
});
