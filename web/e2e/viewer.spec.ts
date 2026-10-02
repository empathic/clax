import { test, expect, type Page } from "@playwright/test";
import { startDaemon, publish } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

/** The artifact's content frame for version `n`, once it exists; locators on it auto-wait. */
async function contentFrame(page: Page, id: string, n: number) {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null).toBe(true);
  return page.frame({ url })!;
}

// Must run first: it expects an empty daemon.
test("gallery shows an empty state then a card", async ({ page }) => {
  await page.goto(`${d.base}/`);
  await expect(page.getByText("When an agent publishes a page, it lands here.")).toBeVisible();
  await publish(d.base, d.token, "Hello Report", { "index.html": "<title>Hello</title><h1>Hi</h1>" });
  await page.reload();
  await expect(page.locator("a.card")).toHaveCount(1);
  await expect(page.locator("a.card")).toContainText("Hello Report");
});

test("viewer renders content with the bridge, and shows a banner on republish", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Live", { "index.html": `<link rel="stylesheet" href="style.css"><h1 id=h>v1</h1><script>document.title = typeof window.claude.use</script>`, "style.css": "#h{color:rgb(0,128,0)}" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = await contentFrame(page, artifact.id, 1);
  await expect(frame.locator("#h")).toHaveText("v1");
  await expect(frame.locator("#h")).toHaveCSS("color", "rgb(0, 128, 0)");
  await expect.poll(async () => await frame.title()).toBe("function");
  const src = (await page.locator("iframe.frame").getAttribute("src")) ?? "";
  const sandbox = await page.locator("iframe.frame").getAttribute("sandbox");
  if (src.includes(".localhost:")) { console.log("frame mode: subdomain"); expect(sandbox).toBeNull(); }
  else { console.log("frame mode: sandboxed fallback"); expect(sandbox).not.toBeNull(); }
  expect(src).toMatch(new RegExp(`(${artifact.id}\\.localhost:\\d+/v/1/|/c/${artifact.id}/v/1/)$`));
  await publish(d.base, d.token, "Live", { "index.html": "<h1 id=h>v2</h1>" }, 1, artifact.id);
  await expect(page.getByText("v2 published")).toBeVisible();
  await page.getByRole("button", { name: "Reload" }).click();
  await expect((await contentFrame(page, artifact.id, 2)).locator("#h")).toHaveText("v2");
  await page.selectOption("select", "1");
  await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}/v/1$`));
  await expect((await contentFrame(page, artifact.id, 1)).locator("#h")).toHaveText("v1");
});

test("fallback mode uses the sandboxed path-based frame", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Fallback", { "index.html": "<p id=p>fallback</p>" });
  await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch {} });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const iframe = page.locator("iframe.frame");
  await expect(iframe).toHaveAttribute("sandbox", /allow-scripts/);
  const src = await iframe.getAttribute("src");
  expect(src).toMatch(new RegExp(`/c/${artifact.id}/v/1/`));
  await expect((await contentFrame(page, artifact.id, 1)).locator("#p")).toHaveText("fallback");
  const res = await page.request.get(new URL(src!, d.base).toString());
  expect(res.headers()["content-security-policy"]).toContain("sandbox");
});

test("viewer shows the deleted state", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Doomed", { "index.html": "<p>bye</p>" });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await expect(page.locator("iframe.frame")).toBeVisible();
  const res = await page.request.delete(`${d.base}/api/artifacts/${artifact.id}`, { headers: { authorization: `Bearer ${d.token}` } });
  expect(res.ok()).toBeTruthy();
  await expect(page.getByText("This artifact was deleted")).toBeVisible();
});

test("gallery and viewer fit a phone in dark mode", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Phone", { "index.html": "<p>small</p>" });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ colorScheme: "dark" });
  for (const [path, ready] of [["/", "a.card"], [`/a/${artifact.id}`, "iframe.frame"]]) {
    await page.goto(`${d.base}${path}`);
    await expect(page.locator(ready).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
    const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    const m = bg.match(/rgba?\(([^)]+)\)/);
    expect(m, bg).not.toBeNull();
    const parts = m![1].split(/[,\s/]+/).filter(Boolean).map(Number);
    const alpha = parts.length > 3 ? parts[3] : 1;
    expect(alpha, bg).not.toBe(0);
    expect(Math.max(...parts.slice(0, 3)), bg).toBeLessThan(128);
  }
});
