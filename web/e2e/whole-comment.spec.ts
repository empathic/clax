// A comment on the whole artifact, from the threads panel's New comment box:
// it is posted with no anchor on the page, shows without a pin, reaches the
// agent with its anchor line, is answered and resolved, and carries forward
// to a new version without detaching. With CLAX_WHOLE_SHOTS set to a
// directory, the panel (the thread on the new version, the box open) is
// also photographed at 1280 and 390 px wide, light and dark.
import { mkdirSync } from "node:fs";
import { type Page } from "@playwright/test";
import { test, expect, type Daemon, api, contentFrame, publishAs, publishNext, registerSession, setName } from "./fixtures";
import { settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const SHOTS = process.env.CLAX_WHOLE_SHOTS ?? "";

/** Opens the threads panel if it is closed (a phone shows the page first). */
async function threads(page: Page) {
  const phone = page.locator(".phone-tabs button", { hasText: "Threads" });
  const toggle = (await phone.isVisible()) ? phone : page.locator("button.threads");
  if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".new-comment-open")).toBeVisible();
}

test("a whole-artifact comment from the threads panel goes through the comment loop", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", `whole-e2e-${Math.random().toString(36).slice(2)}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Palette", { "index.html": "<main><h2>Palette</h2><p>Cool blues and greys.</p></main>" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await setName(page, "Alex");
  await contentFrame(page, artifact.id, 1);
  await threads(page);

  await page.locator(".new-comment-open").click();
  const composer = page.locator(".new-comment .composer");
  await expect(composer.locator(".composer-quote")).toHaveText("The whole artifact");
  await expect(composer).toContainText("No screenshot: it is about the whole artifact");
  await expect(composer.locator("textarea")).toBeFocused();
  await page.keyboard.type("Make the colours warmer.");
  await composer.getByRole("button", { name: "Post comment" }).click();
  await expect(composer).toHaveCount(0);

  const card = page.locator(".section-open .thread-card").first();
  await expect(card.locator(".anchor-label")).toHaveText("The whole artifact");
  await expect(card.locator(".thread-num")).toHaveCount(0);
  await expect(page.locator("button.thread-pin")).toHaveCount(0);
  await expect(page.locator(".section-detached .thread-card")).toHaveCount(0);
  const tid = (await card.getAttribute("data-thread"))!;
  const stored = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}`);
  expect(stored.thread.anchor).toMatchObject({ kind: "artifact", file: "", selector: null, quote: null });
  expect(stored.thread.version_n).toBe(1);

  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(card.locator(".waiting")).toContainText("sent, waiting for the agent");
  const fb = await api(d.base, d.token, `/api/sessions/${s.id}/feedback?tier=piggyback`);
  expect(fb.feedback).toHaveLength(1);
  expect(fb.feedback[0]).toMatchObject({ thread_id: tid, author: "Alex", version: 1, clip_path: null, anchor: { kind: "artifact" } });
  expect(fb.text).toContain([
    "Anchored on: the whole artifact  (v1)",
    "Clip: none (the comment is about the whole artifact)",
    `Alex: "Make the colours warmer."`,
  ].join("\n"));

  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Warmed the palette to ambers.", author_kind: "agent" }) });
  await expect(card.locator(".msg.agent")).toContainText("Warmed the palette to ambers.");

  // A new version: the thread carries forward, still with no pin and never Detached.
  await publishNext(d.base, d.token, s.id, artifact.id, 1, { note: "Warmer palette", files: { "index.html": "<main><h2>Palette</h2><p>Warm ambers.</p></main>" } });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await contentFrame(page, artifact.id, 2);
  await threads(page);
  const again = page.locator(`.thread-card[data-thread="${tid}"]`);
  await expect(again).toBeVisible();
  await expect(page.locator(".section-detached .thread-card")).toHaveCount(0);
  await expect(page.locator("button.thread-pin")).toHaveCount(0);

  if (SHOTS) {
    // The thread on v2 under the box, closed and then open, at both widths, in both themes.
    mkdirSync(SHOTS, { recursive: true });
    for (const theme of ["light", "dark"] as const) {
      for (const [w, h] of [[1280, 800], [390, 844]] as const) {
        await page.setViewportSize({ width: w, height: h });
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.goto(`${d.base}/a/${artifact.id}`);
        await contentFrame(page, artifact.id, 2);
        await threads(page);
        await settle(page);
        await page.screenshot({ path: `${SHOTS}/${theme}-${w}-box.png` });
        await page.locator(".new-comment-open").click();
        await page.keyboard.type("Could the headings be a touch larger?");
        await settle(page);
        await page.screenshot({ path: `${SHOTS}/${theme}-${w}.png` });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(w);
      }
    }
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.emulateMedia({ colorScheme: "light" });
  }

  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/resolve`, { method: "POST", session: s.id, body: JSON.stringify({ as: "agent" }) });
  const done = page.locator(".section-resolved .thread-card");
  await expect(done).toHaveCount(1);
  await expect(done.locator(".hist")).toContainText("claude resolved");
});
