// The gallery's New (docs/contract.md "Starting an artifact from a brief")
// on a local daemon: the owner writes a title and a brief, picks a live
// agent, lands on the new artifact's placeholder page, and the agent's
// session receives the brief; with no agent live, the dialog shows a
// prompt to paste into one; a LAN viewer, who may not create, is offered
// no New. With CLAX_E2E_SHOTS set, the last two tests photograph the dialog
// in both states and the placeholder page, desktop and phone, light and
// dark, into CLAX_E2E_SHOTS_DIR (target/new-artifact-shots by default).
import { mkdirSync } from "node:fs";
import { type Page } from "@playwright/test";
import { api, contentFrame, type Daemon, expect, registerSession, test } from "./fixtures";
import { settle } from "./time";

const BRIEF = "A bar chart of Q3 revenue by region, sorted high to low, with last quarter as a faint ghost bar.";
const SHOTS = process.env.CLAX_E2E_SHOTS ? (process.env.CLAX_E2E_SHOTS_DIR ?? "../target/new-artifact-shots") : null;
const SIZES = { desktop: { width: 1440, height: 900 }, phone: { width: 390, height: 844 } } as const;

/** Opens New on the gallery and creates `title` from the brief; the dialog's second step. */
async function create(page: Page, title: string) {
  await page.getByRole("button", { name: "New artifact" }).click();
  const form = page.getByRole("dialog", { name: "New artifact" });
  await form.getByLabel("Title").fill(title);
  await form.getByLabel("Brief").fill(BRIEF);
  await form.getByRole("button", { name: "Create" }).click();
  return page.getByRole("dialog", { name: "Who builds it?" });
}

/** The artifact ID the page shows. */
const shownId = (page: Page) => /\/a\/([a-z0-9]+)$/.exec(new URL(page.url()).pathname)![1];

test("the owner starts an artifact from a brief, picks a live agent and lands on it, and the agent gets the brief", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token, "claude");
  await page.goto(d.base);
  const who = await create(page, "Q3 revenue");
  await expect(who.getByRole("region", { name: "Your agents" })).toBeVisible();
  const pick = who.getByRole("button", { name: new RegExp(s.name ?? "claude") });
  await expect(pick).toBeFocused();
  await pick.click();
  await expect(page).toHaveURL(/\/a\/[a-z0-9]+$/);
  const aid = shownId(page);
  // The placeholder page, titled.
  const frame = await contentFrame(page, aid, 1);
  await expect(frame.locator("h1")).toHaveText("Q3 revenue");
  await expect(frame.locator("body")).toContainText("An agent hasn't built this page yet.");
  await expect(page.locator(".topbar h1")).toHaveText("Q3 revenue");
  await expect(page.locator(".ttl .by")).toHaveText("started from a brief by Viewer");
  // The agent watches the artifact and its session holds the brief.
  const watches = await api(d.base, d.token, `/api/sessions/${s.id}/watches`) as { watches: { artifact_id: string }[] };
  expect(watches.watches.map(w => w.artifact_id)).toContain(aid);
  const fb = await api(d.base, d.token, `/api/sessions/${s.id}/feedback?wait=5`);
  expect(JSON.stringify(fb)).toContain("Q3 revenue by region");
  // The brief is the artifact's one thread, on the whole artifact.
  const t = await api(d.base, d.token, `/api/artifacts/${aid}/threads`) as { threads: { anchor: { kind: string }; comments: { body: string }[] }[] };
  expect(t.threads).toHaveLength(1);
  expect(t.threads[0].anchor.kind).toBe("artifact");
  expect(t.threads[0].comments[0].body).toBe(BRIEF);
});

test("with no agent live, the dialog gives a prompt to paste into one, and opens the artifact", async ({ page, freshDaemon: d }) => {
  await page.goto(d.base);
  const who = await create(page, "Launch notes");
  await expect(who).toContainText("No agent is connected.");
  const prompt = who.getByLabel("Prompt", { exact: true });
  await expect(prompt).toContainText(`${d.base}/a/`);
  await expect(prompt).toContainText("\"Launch notes\"");
  await who.getByRole("button", { name: "Open the artifact" }).click();
  await expect(page).toHaveURL(/\/a\/[a-z0-9]+$/);
  const aid = shownId(page);
  expect(await prompt.count()).toBe(0);
  const frame = await contentFrame(page, aid, 1);
  await expect(frame.locator("h1")).toHaveText("Launch notes");
});

test("a LAN viewer, who may not create, is offered no New", async ({ page, daemon: d }) => {
  await page.route("**/api/token", r => r.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "not_loopback", message: "not a loopback connection" } }) }));
  await page.goto(d.base);
  await expect(page.locator(".gbar .search")).toBeVisible();
  await expect.poll(() => page.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).may_create)).toBe(false);
  await expect(page.getByRole("button", { name: "New artifact" })).toHaveCount(0);
});

/** Photographs `page` as `name` when shots are on. */
async function shot(page: Page, name: string) {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
}

/** Every theme and size, each in a page of its own on `d`. */
async function eachLook(browser: import("@playwright/test").Browser, d: Daemon, fn: (page: Page, look: string) => Promise<void>) {
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of Object.entries(SIZES)) {
      const ctx = await browser.newContext({ viewport, colorScheme: theme, reducedMotion: "reduce", hasTouch: size === "phone" });
      const page = await ctx.newPage();
      await page.goto(d.base);
      await fn(page, `${theme}-${size}`);
      await ctx.close();
    }
  }
}

test("shots: the dialog with agents to pick, and the placeholder page", async ({ browser, freshDaemon: d }) => {
  test.skip(!SHOTS, "set CLAX_E2E_SHOTS to take the screenshots");
  test.setTimeout(120_000);
  await registerSession(d.base, d.token, "claude");
  await registerSession(d.base, d.token, "codex");
  await eachLook(browser, d, async (page, look) => {
    await shot(page, `gallery-${look}`);
    await page.getByRole("button", { name: "New artifact" }).click();
    const form = page.getByRole("dialog", { name: "New artifact" });
    await form.getByLabel("Title").fill("Q3 revenue");
    await form.getByLabel("Brief").fill(BRIEF);
    await shot(page, `dialog-brief-${look}`);
    await form.getByRole("button", { name: "Create" }).click();
    const who = page.getByRole("dialog", { name: "Who builds it?" });
    await expect(who.locator("button.agent")).toHaveCount(2);
    await shot(page, `dialog-agents-${look}`);
    await who.locator("button.agent").first().click();
    await expect(page).toHaveURL(/\/a\/[a-z0-9]+$/);
    const frame = await contentFrame(page, shownId(page), 1);
    await expect(frame.locator("h1")).toHaveText("Q3 revenue");
    await shot(page, `placeholder-${look}`);
  });
});

test("shots: the dialog with no agent live", async ({ browser, freshDaemon: d }) => {
  test.skip(!SHOTS, "set CLAX_E2E_SHOTS to take the screenshots");
  test.setTimeout(120_000);
  await eachLook(browser, d, async (page, look) => {
    await create(page, "Launch notes");
    await expect(page.getByRole("dialog", { name: "Who builds it?" })).toContainText("No agent is connected.");
    await shot(page, `dialog-none-${look}`);
  });
});
