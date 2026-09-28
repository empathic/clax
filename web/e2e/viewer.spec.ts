import { test, expect } from "@playwright/test";
import { startDaemon, publish } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { d = await startDaemon(); });
test.afterAll(async () => { await d.stop(); });

test("gallery shows an empty state then a card", async ({ page }) => {
  await page.goto(`${d.base}/`);
  await expect(page.getByText("No artifacts yet")).toBeVisible();
  await publish(d.base, d.token, "Hello Report", { "index.html": "<title>Hello</title><h1>Hi</h1>" });
  await page.reload();
  await expect(page.locator("a.card")).toHaveCount(1);
  await expect(page.locator("a.card")).toContainText("Hello Report");
});

test("viewer renders content with the bridge, and shows a banner on republish", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Live", { "index.html": "<h1 id=h>v1</h1><script>document.title = typeof window.claude.use</script>" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = page.frameLocator("iframe.frame");
  await expect(frame.locator("#h")).toHaveText("v1");
  await expect.poll(async () => await page.frames()[1]?.title()).toBe("function");
  const src = await page.locator("iframe.frame").getAttribute("src");
  expect(src).toMatch(new RegExp(`(${artifact.id}\\.localhost:\\d+/v/1/|/c/${artifact.id}/v/1/)$`));
  await publish(d.base, d.token, "Live", { "index.html": "<h1 id=h>v2</h1>" }, 1, artifact.id);
  await expect(page.getByText("v2 published")).toBeVisible({ timeout: 5000 });
  await page.getByRole("button", { name: "Reload" }).click();
  await expect(page.frameLocator("iframe.frame").locator("#h")).toHaveText("v2");
  await page.selectOption("select", "1");
  await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}/v/1$`));
  await expect(page.frameLocator("iframe.frame").locator("#h")).toHaveText("v1");
});

test("fallback mode uses the sandboxed path-based frame", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Fallback", { "index.html": "<p id=p>fallback</p>" });
  await page.addInitScript(() => { try { sessionStorage.setItem("artifax.origin-ok", "0"); } catch {} });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const iframe = page.locator("iframe.frame");
  await expect(iframe).toHaveAttribute("sandbox", /allow-scripts/);
  const src = await iframe.getAttribute("src");
  expect(src).toMatch(new RegExp(`/c/${artifact.id}/v/1/`));
  await expect(page.frameLocator("iframe.frame").locator("#p")).toHaveText("fallback");
  const res = await page.request.get(new URL(src!, d.base).toString());
  expect(res.headers()["content-security-policy"]).toContain("sandbox");
});

test("viewer shows the deleted state", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Doomed", { "index.html": "<p>bye</p>" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await expect(page.locator("iframe.frame")).toBeVisible();
  const res = await page.request.delete(`${d.base}/api/artifacts/${artifact.id}`, { headers: { authorization: `Bearer ${d.token}` } });
  expect(res.ok()).toBeTruthy();
  await expect(page.getByText("This artifact was deleted")).toBeVisible({ timeout: 5000 });
});

test("gallery and viewer fit a phone in dark mode", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Phone", { "index.html": "<p>small</p>" });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ colorScheme: "dark" });
  for (const path of ["/", `/a/${artifact.id}`]) {
    await page.goto(`${d.base}${path}`);
    await expect(page.locator("body")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
    expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).not.toBe("rgb(255, 255, 255)");
  }
});
