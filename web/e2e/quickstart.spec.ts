import { test, expect, startDaemon } from "./fixtures";
import { SECRET, hostedDaemon } from "./hosted-helpers";

// Each test starts a daemon of its own: it judges a brand-new gallery. Only a
// hosted site's galleries are seeded with the quickstart; a local home is not.
test("a new local home's gallery is empty, with the quickstart on", async ({ page }) => {
  const own = await startDaemon({ env: { CLAX_GALLERY__QUICKSTART: "true" } });
  try {
    await page.goto(`${own.base}/`);
    await expect(page.getByText("When an agent publishes a page, it lands here.")).toBeVisible();
    await expect(page.locator("a.card")).toHaveCount(0);
  } finally {
    await own.stop();
  }
});

test("an invited person's hosted gallery shows the quickstart, pinned", async ({ browser }) => {
  const { d, origin } = await hostedDaemon("", { CLAX_GALLERY__QUICKSTART: "true" });
  try {
    const owner = await (await browser.newContext()).newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const url = await owner.evaluate(async () => {
      const res = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Mia" }) });
      return (await res.json()).invite.url as string;
    });
    const mia = await (await browser.newContext()).newPage();
    await mia.goto(url);
    await mia.getByRole("button", { name: "Join as Mia" }).click();
    await mia.waitForURL(`${origin}/`);
    const card = mia.locator("a.card");
    await expect(card).toHaveCount(1);
    await expect(card).toContainText("Clax Quickstart");
    // Pinned shows as the Unpin button once the token is known, else as the star.
    await expect(mia.locator('.card-tools button[title="Unpin"], .card .pin')).not.toHaveCount(0);
    await expect(mia.locator(".empty-gallery")).toHaveCount(0);

    await card.click();
    const frame = mia.frameLocator("iframe").first();
    await expect(frame.locator("h1")).toHaveText("Your agents build pages. Clax gives them a home.");
    // A seeded copy ships without the ideas page, so its tab stays hidden.
    await expect(frame.locator("#tab-ideas")).toBeHidden();
  } finally {
    await d.stop();
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
