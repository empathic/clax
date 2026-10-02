import { test, expect, type Page } from "@playwright/test";
import { api, publish, publishAs, registerSession, startDaemon, setName } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = `<main><h2>Quarterly goals</h2><p>Grow revenue and keep costs flat this quarter.</p></main>`;

async function contentFrame(page: Page, id: string, n: number) {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null).toBe(true);
  return page.frame({ url })!;
}

async function pickHeading(page: Page, id: string, n: number) {
  const frame = await contentFrame(page, id, n);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "true");
  await frame.locator("h2").hover();
  await frame.locator("h2").click();
  await expect(page.locator(".composer")).toBeVisible();
  return frame;
}

test("element thread: pick, compose, pin, send, agent reply, resolve", async ({ page }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Goals", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await pickHeading(page, artifact.id, 1);
  const composer = page.locator(".composer");
  await expect(composer.locator(".composer-quote")).toContainText("Quarterly goals");
  await expect(composer.locator("img.clip")).toBeVisible();
  await composer.locator("textarea").fill("Make this two columns.");
  await composer.getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".section-open .thread-card").first();
  await expect(card).toContainText("Make this two columns.");
  await expect(page.locator("button.thread-pin")).toHaveCount(1);
  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(card.locator(".waiting")).toContainText("sent, waiting for the agent");
  await expect(card.locator(".waiting")).toContainText("waiting on the end of its turn");
  const tid = (await card.getAttribute("data-thread"))!;
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Done: two columns.", author_kind: "agent" }) });
  await expect(card.locator(".msg.agent .author")).toContainText("claude");
  await expect(card.locator(".waiting")).toHaveText("seen by the agent");
  await card.getByRole("button", { name: "Resolve" }).click();
  await expect(page.locator(".section-resolved .thread-card")).toHaveCount(1);
  await expect(page.locator(".section-resolved .thread-card .hist")).toContainText("Viewer resolved");
  await expect(page.locator("button.thread-pin")).toHaveCount(0);
});

test("range thread quotes the selected text", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Range", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = await contentFrame(page, artifact.id, 1);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  const box = (await frame.locator("p").boundingBox())!;
  await page.mouse.move(box.x + 3, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 90, box.y + box.height / 2, { steps: 5 });
  await page.mouse.up();
  const quote = page.locator(".composer .composer-quote");
  await expect(quote).not.toBeEmpty();
  expect("Grow revenue and keep costs flat this quarter.").toContain((await quote.textContent())!.replace(/[«»]/g, "").trim());
});

test("republish re-anchors kept elements and detaches removed ones", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Detach", { "index.html": "<main><h2>Keep me</h2><p>Gone soon</p></main>" });
  const mk = async (selector: string, quote: string, body: string) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", body);
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`, { method: "POST", body: form })).status).toBe(201);
  };
  await mk("body > main > h2", "Keep me", "heading note");
  await mk("body > main > p", "Gone soon", "paragraph note");
  await publish(d.base, d.token, "Detach", { "index.html": "<main><h2>Keep me</h2></main>" }, 1, artifact.id);
  await page.goto(`${d.base}/a/${artifact.id}`);
  await contentFrame(page, artifact.id, 2);
  await expect(page.locator(".section-open .thread-card")).toContainText("heading note");
  await expect(page.locator(".section-detached .thread-card")).toContainText("paragraph note");
  await expect(page.locator("button.thread-pin")).toHaveCount(1);
});

test("republish while composing records the picked version", async ({ page }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Race", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await pickHeading(page, artifact.id, 1);
  await publishAs(d.base, d.token, s.id, "Race", { "index.html": PAGE.replace("costs flat", "costs down") }, 1, artifact.id);
  await expect(page.locator(".who .sum b.l1")).toHaveText("v2 published");
  await page.locator(".composer textarea").fill("composed on v1");
  await page.locator(".composer").getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".thread-card").filter({ hasText: "composed on v1" });
  await expect(card).toHaveCount(1);
  const tid = (await card.getAttribute("data-thread"))!;
  const t = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}`);
  expect(t.thread.version_n).toBe(1);
  await page.getByRole("button", { name: "Reload" }).click();
  await contentFrame(page, artifact.id, 2);
  await expect(page.locator(".section-open .thread-card").filter({ hasText: "composed on v1" })).toHaveCount(1);
});

test("the viewer name attributes comments", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Named", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await setName(page, "Alex");
  await pickHeading(page, artifact.id, 1);
  await page.locator(".composer textarea").fill("named note");
  await page.locator(".composer").getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".thread-card").filter({ hasText: "named note" });
  await expect(card).toContainText("Alex");
  await card.getByRole("button", { name: "Resolve" }).click();
  await expect(page.locator(".section-resolved .thread-card .hist")).toContainText("Alex resolved");
});

test("sandboxed frames support comment mode too", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Sandboxed", { "index.html": PAGE });
  await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch {} });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await expect(page.locator("iframe.frame")).toHaveAttribute("sandbox", /allow-scripts/);
  await pickHeading(page, artifact.id, 1);
  await page.locator(".composer textarea").fill("sandbox note");
  await page.locator(".composer").getByRole("button", { name: "Post comment" }).click();
  await expect(page.locator(".section-open .thread-card").filter({ hasText: "sandbox note" })).toHaveCount(1);
});

test("a failed send shows in the banner and clears on the next success", async ({ page }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Failing send", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await pickHeading(page, artifact.id, 1);
  await page.locator(".composer textarea").fill("send me");
  await page.locator(".composer").getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".section-open .thread-card").filter({ hasText: "send me" });
  await page.route("**/threads/*/send", route => route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: { code: "internal", message: "boom" } }) }));
  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(page.locator(".banner.notice")).toHaveText(/Could not send to the agent: 500 boom/);
  await page.unroute("**/threads/*/send");
  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(card.locator(".waiting")).toContainText("sent, waiting for the agent");
  await expect(page.locator(".banner.notice")).toHaveCount(0);
});

test("the thread panel fits a phone", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Phone threads", { "index.html": PAGE });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await page.getByRole("button", { name: /^Threads/ }).click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
});

test("Escape in the shell cancels comment mode", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Escape", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const frame = await contentFrame(page, artifact.id, 1);
  const toggle = page.getByRole("button", { name: "Comment", exact: true });
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await toggle.focus();
  await page.keyboard.press("Escape");
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  // The bridge left comment mode too: a click in the frame picks nothing.
  await frame.locator("h2").click();
  await page.waitForTimeout(300);
  await expect(page.locator(".composer")).toHaveCount(0);
});

test("thread cards are reachable and selectable from the keyboard", async ({ page }) => {
  const { artifact } = await publish(d.base, d.token, "Keyboard", { "index.html": PAGE });
  for (const [selector, quote, body] of [["body > main > h2", "Quarterly goals", "first"], ["body > main > p", "Grow revenue", "second"]]) {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", body);
    form.set("version", "1");
    expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`, { method: "POST", body: form })).status).toBe(201);
  }
  await page.goto(`${d.base}/a/${artifact.id}`);
  await contentFrame(page, artifact.id, 1);
  const first = page.locator(".thread-card").filter({ hasText: "first" });
  const second = page.locator(".thread-card").filter({ hasText: "second" });
  await first.locator("button.card-head").focus();
  await page.keyboard.press("Enter");
  await expect(first).toHaveClass(/selected/);
  await page.keyboard.press("Tab"); // Resolve
  await page.keyboard.press("Tab"); // Send to the agent
  await page.keyboard.press("Tab"); // Reply input
  await page.keyboard.press("Tab"); // Reply button
  await page.keyboard.press("Tab"); // the second card's box, for a batch send
  await expect(second.locator(".thread-check")).toBeFocused();
  await page.keyboard.press("Tab"); // the second card's header
  await expect(second.locator("button.card-head")).toBeFocused();
  await page.keyboard.press("Space");
  await expect(second).toHaveClass(/selected/);
  await expect(first).not.toHaveClass(/selected/);
  const toggle = page.getByRole("button", { name: "Comment", exact: true });
  await toggle.click();
  await second.locator("button.card-head").focus();
  await page.keyboard.press("Escape");
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
});
