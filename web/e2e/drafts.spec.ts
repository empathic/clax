// Agents drafting live (hosted spec 2026-10-11-clax-dev-hosted-design
// §16.4): a scripted agent drafts on an artifact; the viewer sees the
// banner, shows the draft with its changes tinted in the agents' colour,
// goes back to the version, and sees the draft clear when the agent
// publishes.
import type { Frame, Page } from "@playwright/test";
import { test, expect, type Daemon, api, contentFrame, openArtifact, publishAs, registerSession, streamLive } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = `<!doctype html><html><head><title>Plan</title><link rel="stylesheet" href="style.css"></head>
<body><main><h1>Quarterly plan</h1><p>Ship the beta</p><p>Hire two people</p></main></body></html>`;
const DRAFT = PAGE.replace("Ship the beta", "Ship the beta in May");
const CSS = "body { font: 16px/1.5 system-ui; margin: 32px; } h1 { color: rgb(10, 20, 30); }";

const putDraft = (sid: string, aid: string, html: string, note?: string) =>
  api(d.base, d.token, `/api/sessions/${sid}/drafts/${aid}`, { method: "PUT", body: JSON.stringify({ files: { "index.html": { content: html, encoding: "utf8" } }, note }) });

/** The token of the live draft on `aid`, as the owner lists it (by hand: the token, no session). */
async function liveToken(aid: string): Promise<string> {
  const r = await api(d.base, d.token, `/api/artifacts/${aid}/drafts`);
  return r.drafts[0].token as string;
}

/** The frame showing the draft revision `token` (any, when absent) of `aid`, once loaded. */
async function draftFrame(page: Page, aid: string, token = "[A-Za-z0-9_-]{22}"): Promise<Frame> {
  const url = new RegExp(`(${aid}\\.localhost:\\d+|/c/${aid})/draft/a_[^/]+/${token}/`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 10_000 }).toBe(true);
  const f = page.frame({ url })!;
  await f.waitForLoadState("load");
  return f;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a viewer sees an agent drafting, shows the draft tinted, goes back, and sees it clear on publish`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `draft-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Drafts ${mode}`, { "index.html": PAGE, "style.css": CSS });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await streamLive(page);
    const banner = page.locator(".banner.draft");
    await expect(banner).toHaveCount(0);

    await putDraft(s.id, artifact.id, DRAFT, "rewording the plan");
    await expect(banner).toContainText("claude is drafting…");
    await expect(banner).toContainText("rewording the plan");
    // Drafting is working: the top bar does not call the agent idle.
    await expect(page.locator(".who .sum b.l1")).toHaveText("claude working");
    await expect(page.locator(".who .sum")).not.toContainText("idle");
    // A draft never welcomes an unnamed caller: the listing refuses no credential.
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/drafts`)).status).toBe(403);
    await banner.getByRole("button", { name: "Show draft" }).click();

    const f = await draftFrame(page, artifact.id);
    const changed = f.locator("[data-clax-draft-changed]");
    await expect(changed).toHaveCount(1);
    await expect(changed).toHaveText("Ship the beta in May");
    // Tinted in the agents' colour (the light theme's --agent, #2f6f2a); the
    // rest of the page untouched, its stylesheet from the version.
    await expect.poll(() => changed.evaluate(el => getComputedStyle(el).outlineColor)).toBe("rgb(47, 111, 42)");
    expect(await f.locator("h1").evaluate(el => getComputedStyle(el).outlineStyle)).toBe("none");
    expect(await f.locator("h1").evaluate(el => getComputedStyle(el).color)).toBe("rgb(10, 20, 30)");
    await expect(banner).toContainText("Showing claude’s draft over v1 · 1 change highlighted");
    // Comments go on published versions.
    await expect(page.locator(".topbar button.comment")).toBeDisabled();

    await banner.getByRole("button", { name: "Back to v1" }).click();
    const v1 = await contentFrame(page, artifact.id, 1);
    await expect(v1.locator("p").first()).toHaveText("Ship the beta");
    await expect(banner).toContainText("claude is drafting…");
    await expect(page.locator(".topbar button.comment")).toBeEnabled();

    // A revision while shown is shown at once.
    await banner.getByRole("button", { name: "Show draft" }).click();
    await draftFrame(page, artifact.id);
    await putDraft(s.id, artifact.id, DRAFT.replace("Hire two people", "Hire three people"));
    const revised = await draftFrame(page, artifact.id, await liveToken(artifact.id));
    await expect(revised.locator("[data-clax-draft-changed]")).toHaveCount(2);
    await expect(banner).toContainText("2 changes highlighted");

    // The agent publishes: the draft ends, the view goes back to v1 and offers v2.
    await publishAs(d.base, d.token, s.id, `Drafts ${mode}`, { "index.html": DRAFT, "style.css": CSS }, 1, artifact.id);
    await expect(banner).toHaveCount(0);
    await contentFrame(page, artifact.id, 1);
    await expect(page.locator(".topbar button.reload")).toBeVisible();
  });
}

test("at phone width in dark mode the banner fits and the draft is tinted in the dark agent colour", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "codex", "draft-phone");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title for a phone-width draft check", { "index.html": PAGE, "style.css": CSS });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await streamLive(page);
  await putDraft(s.id, artifact.id, DRAFT, "rebuilding the quarterly plan with the new numbers");
  const banner = page.locator(".banner.draft");
  await expect(banner).toContainText("codex is drafting…");
  const box = (await banner.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await banner.getByRole("button", { name: "Show draft" }).click();
  const f = await draftFrame(page, artifact.id);
  await expect.poll(() => f.locator("[data-clax-draft-changed]").evaluate(el => getComputedStyle(el).outlineColor)).toBe("rgb(124, 195, 111)");
  const shown = (await banner.boundingBox())!;
  expect(shown.x + shown.width).toBeLessThanOrEqual(390);
});
