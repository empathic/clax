import { test, expect, startDaemon } from "./fixtures";

// Each test starts a daemon of its own: it judges a brand-new home's gallery.
test("a new home's gallery shows the quickstart, pinned", async ({ page }) => {
  const own = await startDaemon({ env: { CLAX_GALLERY__QUICKSTART: "true" } });
  try {
    await page.goto(`${own.base}/`);
    const card = page.locator("a.card");
    await expect(card).toHaveCount(1);
    await expect(card).toContainText("Clax Quickstart");
    // Pinned shows as the Unpin button once the token is known, else as the star.
    await expect(page.locator('.card-tools button[title="Unpin"], .card .pin')).not.toHaveCount(0);
    await expect(page.locator(".empty-gallery")).toHaveCount(0);

    await card.click();
    const frame = page.frameLocator("iframe").first();
    await expect(frame.locator("h1")).toHaveText("Your agents build pages. Clax gives them a home.");
    // A seeded copy ships without the ideas page, so its tab stays hidden.
    await expect(frame.locator("#tab-ideas")).toBeHidden();
  } finally {
    await own.stop();
  }
});

test("the switch off leaves the gallery empty", async ({ page }) => {
  const own = await startDaemon();
  try {
    await page.goto(`${own.base}/`);
    await expect(page.getByText("When an agent publishes a page, it lands here.")).toBeVisible();
  } finally {
    await own.stop();
  }
});
