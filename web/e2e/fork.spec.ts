import { test, expect, type Daemon, publish } from "./fixtures";
import { guestOn, hostedDaemon, ownerOf, shareByLink } from "./hosted-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

/** An artifact titled `title` with two versions; its ID. */
async function twoVersions(title: string): Promise<string> {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": "<main><h2>One</h2></main>" });
  await publish(d.base, d.token, title, { "index.html": "<main><h2>Two</h2></main>" }, 1, artifact.id);
  return artifact.id;
}

test("the owner forks the version shown, lands on the fork and sees where it came from", async ({ page }) => {
  const id = await twoVersions("Fork me");
  await page.goto(`${d.base}/a/${id}/v/1`);
  const fork = page.locator(".topbar button.fork");
  await expect(fork).toBeVisible();
  await expect(fork).toHaveAttribute("title", "Fork v1 into an artifact of your own");
  await fork.click();
  await expect(page).not.toHaveURL(new RegExp(`/a/${id}`));
  const fid = /\/a\/([a-z0-9]+)$/.exec(new URL(page.url()).pathname)![1];
  expect(fid).not.toBe(id);
  await expect(page.locator(".topbar h1")).toHaveText("Fork me");
  const lineage = page.locator(".ttl .by a.lineage");
  await expect(lineage).toHaveText("Forked from Fork me v1");
  await expect(lineage).toHaveAttribute("href", `/a/${id}/v/1`);
  const body = await (await fetch(`${d.base}/api/artifacts/${fid}/versions/1/files/index.html`)).text();
  expect(body).toBe("<main><h2>One</h2></main>");
  // The link goes back to the source's version.
  await lineage.click();
  await expect(page).toHaveURL(new RegExp(`/a/${id}/v/1$`));
  await expect(page.locator(".ttl .by .lineage")).toHaveCount(0);
});

test("at phone width Fork is in the more menu and the source stays named", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  const id = await twoVersions("Phone fork");
  await page.goto(`${d.base}/a/${id}`);
  await expect(page.locator(".topbar button.fork")).toBeHidden();
  await page.locator(".topbar button[aria-label=More]").click();
  await page.getByRole("menuitem", { name: "Fork" }).click();
  await expect(page).not.toHaveURL(new RegExp(`/a/${id}`));
  await expect(page.locator(".ttl .by a.lineage")).toHaveText("Forked from Phone fork v2");
  await expect(page.locator(".ttl .by a.lineage")).toBeVisible();
});

// A viewer who is not the owner: a hosted daemon's guest on a page shared by
// link (a local daemon frames pages for the owner alone).
test("a guest, who may not fork, is offered no Fork", async ({ browser }) => {
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await publish(h.base, h.token, "Not yours", { "index.html": "<main><h2>One</h2></main>" });
    await shareByLink(await ownerOf(browser, origin), artifact.id);
    const { page } = await guestOn(browser, origin, artifact.id);
    // The more menu has loaded, and Fork stays out.
    await expect(page.locator(".topbar button[aria-label=More]")).toBeVisible();
    await expect(page.locator(".topbar button.fork")).toHaveCount(0);
  } finally {
    await h.stop();
  }
});
