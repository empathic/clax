import { test, expect, type Daemon, contentFrame, openArtifact, publishAs, publishNext, registerSession } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the top bar reads Echo, and comment mode shows the red-orange rule`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `echo-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Echo ${mode}`, { "index.html": "<main><h2>Goals</h2></main>" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator(".topbar h1")).toHaveText(`Echo ${mode}`);
    await expect(page.locator(".topbar .by")).toHaveText("published by claude");
    // The title is the system sans; the page asks for no font file.
    const h1Font = await page.locator(".topbar h1").evaluate(e => getComputedStyle(e).fontFamily);
    expect(h1Font).toMatch(/^ui-sans-serif, -apple-system/);
    expect(await page.evaluate(() => performance.getEntriesByType("resource").filter(r => /\.(woff2?|ttf|otf)(\?|$)/.test(r.name)).length)).toBe(0);
    expect(await page.evaluate(() => document.fonts.size)).toBe(0);
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    await comment.click();
    await expect(comment).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".topbar")).toHaveClass(/commenting/);
    // The rule is --you, the people's red-orange.
    const you = await page.evaluate(() => { const p = document.body.appendChild(document.createElement("i")); p.style.color = "var(--you)"; const c = getComputedStyle(p).color; p.remove(); return c; });
    expect(you).toBe("rgb(224, 83, 47)");
    expect(await page.locator(".topbar").evaluate(e => getComputedStyle(e).boxShadow)).toBe(`${you} 0px -3px 0px 0px inset`);
    await page.keyboard.press("Escape");
    await expect(page.locator(".topbar")).not.toHaveClass(/commenting/);
  });
}

test("at phone width the bar keeps the mark, title, Comment and the more menu in one 56px row, and tabs at the foot switch to threads", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-phone");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title that has to fit a phone without pushing anything sideways", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.setViewportSize({ width: 390, height: 844 });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator(".topbar a.home")).toBeVisible();
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "More", exact: true })).toBeVisible();
  await expect(page.locator(".topbar .vbtn")).toBeHidden();
  expect((await page.locator(".topbar").boundingBox())!.height).toBe(56);
  const tabs = page.getByRole("group", { name: "Page or threads" });
  await tabs.getByRole("button", { name: /Threads/ }).click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
  // The tabs come after the threads in the document, as on the screen.
  expect(await page.evaluate(() => {
    const side = document.querySelector("aside.sidebar")!, foot = document.querySelector(".phone-tabs")!;
    return !!(side.compareDocumentPosition(foot) & Node.DOCUMENT_POSITION_FOLLOWING) && !document.querySelector(".topbar .phone-tabs");
  })).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test("on a phone the more menu still opens raw and copies the link, and the gallery link is a 44px target", async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const page = await context.newPage();
  const { artifact } = await publishAs(d.base, d.token, (await registerSession(d.base, d.token, "claude", "echo-phone-more")).id, "Phone more", { "index.html": "<main><h2>Goals</h2></main>" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const home = (await page.locator(".topbar a.home").boundingBox())!;
  expect(Math.min(home.width, home.height)).toBeGreaterThanOrEqual(44);
  await page.getByRole("button", { name: "More", exact: true }).click();
  const raw = page.getByRole("menuitem", { name: "Open raw" });
  await expect(raw).toBeVisible();
  expect(await raw.getAttribute("href")).toBe(await page.locator("iframe.frame").getAttribute("src"));
  await page.getByRole("menuitem", { name: "Copy link" }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(`/a/${artifact.id}`);
  await context.close();
});

test("at phone width the more menu's Versions item opens the version menu as a sheet", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-phone-versions");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Phone versions", { "index.html": "<main><h2>Goals</h2></main>" });
  await publishNext(d.base, d.token, s.id, artifact.id, 1, {});
  await page.setViewportSize({ width: 390, height: 844 });
  await openArtifact(page, d.base, artifact.id, 2, "subdomain");
  await expect(page.locator(".topbar .vbtn")).toBeHidden();
  await page.getByRole("button", { name: "More", exact: true }).click();
  await page.getByRole("menuitem", { name: "Versions" }).click();
  const sheet = page.getByRole("dialog", { name: "Versions" });
  await expect(sheet).toBeVisible();
  await expect(sheet.locator(".vrow")).toHaveCount(2);
  const box = (await sheet.boundingBox())!;
  expect(box.x).toBe(0);
  expect(box.width).toBe(390);
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test("the more menu opens raw and copies the link, and Escape closes it with focus back on its button", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const { artifact } = await publishAs(d.base, d.token, (await registerSession(d.base, d.token, "claude", "echo-more")).id, "Echo more", { "index.html": "<main><h2>Goals</h2></main>" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const more = page.getByRole("button", { name: "More", exact: true });
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
  // C and ? typed in the open menu do nothing in the shell; Tab leaves it and closes it.
  await more.click();
  await expect(raw).toBeFocused();
  await page.keyboard.press("c");
  await page.keyboard.press("Shift+?");
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("Tab");
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(more).not.toBeFocused();
  // Open raw opens a new tab and closes the menu with focus back on its button.
  await more.click();
  const [popup] = await Promise.all([page.context().waitForEvent("page"), raw.click()]);
  await popup.close();
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(more).toBeFocused();
});

test("the sidebar holds its width from the first paint, so the frame is laid out once, however late the thread list's code arrives", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-reserve");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Echo reserve", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.setViewportSize({ width: 1440, height: 900 });
  // The thread list's chunk arrives 300 ms late.
  await page.route("**/ShellSidebar-*.js", async r => { await new Promise(f => setTimeout(f, 300)); await r.continue(); });
  await page.addInitScript(() => {
    const widths: number[] = [];
    (window as unknown as { frameWidths: number[] }).frameWidths = widths;
    new MutationObserver((_, o) => {
      const f = document.querySelector("iframe.frame");
      if (!f) return;
      o.disconnect();
      new ResizeObserver(() => widths.push(Math.round(f.getBoundingClientRect().width))).observe(f);
    }).observe(document, { childList: true, subtree: true });
  });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator("aside.sidebar .gh")).toHaveCount(3);
  expect(new Set(await page.evaluate(() => (window as unknown as { frameWidths: number[] }).frameWidths)).size).toBe(1);
  expect((await page.locator("aside.sidebar").boundingBox())!.width).toBe(392);
});

test("at phone width the thread list's code is fetched after the first paint, before the first Threads tap", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-prefetch");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Echo prefetch", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.setViewportSize({ width: 390, height: 844 });
  const fetched = page.waitForRequest(/\/ShellSidebar-[^/]*\.js$/);
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await fetched;
  await expect(page.locator("aside.sidebar")).toHaveCount(0);
});

test("the tenth version, viewed first in this browser, reads rally of 10 in the top bar once, and the gallery card carries the chip", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-rally");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Echo rally", { "index.html": "<main><h2>Goals</h2></main>" });
  for (let v = 1; v < 10; v++) await publishNext(d.base, d.token, s.id, artifact.id, v, {});
  await openArtifact(page, d.base, artifact.id, 10, "subdomain");
  await expect(page.locator(".who .sum .l2")).toContainText("rally of 10");
  await page.reload();
  await contentFrame(page, artifact.id, 10);
  await expect(page.locator(".who .sum .l2")).toHaveText(/open thread/);
  await expect(page.locator(".who .sum .l2")).not.toContainText("rally of 10");
  await page.goto(`${d.base}/`);
  const card = page.locator(".card-wrap").filter({ has: page.getByRole("heading", { name: "Echo rally" }) });
  await expect(card.locator(".chip.rally")).toHaveText("rally of 10");
});
