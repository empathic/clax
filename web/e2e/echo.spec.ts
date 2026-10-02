import { test, expect } from "@playwright/test";
import { openArtifact, publishAs, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the top bar reads Echo, and comment mode shows the red-orange rule`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `echo-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Echo ${mode}`, { "index.html": "<main><h2>Goals</h2></main>" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator(".topbar h1")).toHaveText(`Echo ${mode}`);
    await expect(page.locator(".topbar .by")).toHaveText("published by claude");
    const h1Font = await page.locator(".topbar h1").evaluate(e => getComputedStyle(e).fontFamily);
    expect(h1Font).toContain("IBM Plex Sans Condensed");
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    await comment.click();
    await expect(comment).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".topbar")).toHaveClass(/commenting/);
    expect(await page.locator(".topbar").evaluate(e => getComputedStyle(e).boxShadow)).toMatch(/ 0px -3px 0px 0px inset$/);
    await page.keyboard.press("Escape");
    await expect(page.locator(".topbar")).not.toHaveClass(/commenting/);
  });
}

test("at phone width the bar keeps the mark, title and Comment, and tabs switch to threads", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-phone");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title that has to fit a phone without pushing anything sideways", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.setViewportSize({ width: 390, height: 844 });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator(".topbar a.home")).toBeVisible();
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toBeVisible();
  await expect(page.locator(".topbar select.version")).toBeHidden();
  await page.getByRole("navigation", { name: "Page or threads" }).getByRole("button", { name: /Threads/ }).click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test("the more menu opens raw and copies the link, and Escape closes it with focus back on its button", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const { artifact } = await publishAs(d.base, d.token, (await registerSession(d.base, d.token, "claude", "echo-more")).id, "Echo more", { "index.html": "<main><h2>Goals</h2></main>" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const more = page.getByRole("button", { name: "Open raw or copy link" });
  await more.click();
  const raw = page.getByRole("menuitem", { name: "Open raw" });
  await expect(raw).toBeFocused();
  expect(await raw.getAttribute("href")).toBe(await page.locator("iframe.frame").getAttribute("src"));
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(more).toBeFocused();
  await more.click();
  await page.getByRole("menuitem", { name: "Copy link" }).click();
  await expect(page.getByRole("menu")).toHaveCount(0);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(`/a/${artifact.id}`);
});
