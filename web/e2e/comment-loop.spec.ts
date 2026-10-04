import { type Page } from "@playwright/test";
import { readFileSync, existsSync } from "node:fs";
import { test, expect, type Daemon, api, publishAs, registerSession, setName } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

async function contentFrame(page: Page, id: string, n: number) {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) test(`${mode}: comment in the browser, the agent receives it, replies, and resolves`, async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", `loop-e2e-${mode}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", {
    "index.html": "<main><h2>Quarterly goals</h2><ul><li>Ship</li><li>Grow</li><li>Drop this</li></ul></main>",
  });
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage blocked */ } });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await setName(page, "Alex");

  // Browser side, for real: comment mode, pick, compose, send.
  const frame = await contentFrame(page, artifact.id, 1);
  expect(frame.url()).toContain(mode === "subdomain" ? `${artifact.id}.localhost:` : `/c/${artifact.id}/v/1/`);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await frame.locator("h2").hover();
  await expect(frame.locator("clax-overlay .o")).toBeVisible();
  await frame.locator("h2").click();
  const composer = page.locator(".composer");
  await expect(composer.locator("img.clip")).toBeVisible();
  await composer.locator("textarea").fill("Make this a two-column layout and drop the third bullet.");
  await composer.getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".section-open .thread-card").first();
  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(card.locator(".waiting")).toContainText("sent, waiting for the agent");
  await expect(card.locator(".waiting")).toContainText("waiting on the agent finishing its current work");
  const tid = (await card.getAttribute("data-thread"))!;

  // Agent side: the next tool result's feedback, exactly as the shim fetches it.
  const fb = await api(d.base, d.token, `/api/sessions/${s.id}/feedback?tier=piggyback`);
  expect(fb.feedback).toHaveLength(1);
  expect(fb.feedback[0]).toMatchObject({ thread_id: tid, author: "Alex", version: 1, artifact_title: "Quarterly Review" });
  expect(fb.text).toContain(`Alex: "Make this a two-column layout and drop the third bullet."`);
  expect(fb.text).toContain("Anchored on: body > main > h2  «Quarterly goals»  (v1)");
  const clip = fb.feedback[0].clip_path as string;
  expect(existsSync(clip)).toBe(true);
  expect([...readFileSync(clip).subarray(0, 8)]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  // Taking the comment marks the agent working on it, which replaces the waiting line.
  await expect(card.locator(".st.ag")).toContainText("claude is working on it");
  await expect(card.locator(".waiting")).toHaveCount(0);

  // The agent replies and resolves; the browser shows both live.
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Done: two columns.", author_kind: "agent" }) });
  await expect(card.locator(".msg.agent .author")).toContainText("claude");
  // Replying to the only thread the record named ends it.
  await expect(card.locator(".st.ag")).toHaveCount(0);
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/resolve`, { method: "POST", session: s.id, body: JSON.stringify({ as: "agent" }) });
  const done = page.locator(".section-resolved .thread-card");
  await expect(done).toHaveCount(1);
  await expect(done).toContainText("Done: two columns.");
  await expect(done.locator(".hist")).toContainText("claude resolved");
  await expect(page.locator("button.thread-pin")).toHaveCount(0);
});
