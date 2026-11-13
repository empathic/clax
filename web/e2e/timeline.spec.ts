// The artifact view's sidebar as a timeline (spec 2026-11-13, "Timeline
// sidebar"): it opens on the Timeline tab, where each thread is a row under
// the newest line about it and opens into its card; the Threads tab keeps
// the thread list, and the browser remembers the choice; closed while
// docked left or right, the sidebar leaves a 56 px strip on that edge that
// shows it again, and none for an artifact with no threads.
//
// Every context's sidebar opens on the thread list unless a choice is
// stored (`trustContext`, fixtures.ts), so each test here stores the
// Timeline itself, once per tab, before the first load: a reload keeps
// whatever the test chose since.
import { createServer } from "node:net";
import type { Locator, Page } from "@playwright/test";
import { NO_KEY_CONFIG, test, expect, type Daemon, api, openArtifact, postThread, publish, publishAs, publishNext, reach, registerSession, startDaemon, streamLive } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

/** Stores `prefs` in the daemon's origin before the tab's first load, and
 * never again in that tab, so a reload keeps what the page stored since. */
async function prefer(page: Page, prefs: Record<string, string>) {
  await page.addInitScript(p => {
    try {
      if (sessionStorage.getItem("e2e.prefs") !== null) return;
      for (const [k, v] of Object.entries(p)) localStorage.setItem(k, v);
      sessionStorage.setItem("e2e.prefs", "1");
    } catch { /* storage refused */ }
  }, prefs);
}
const TIMELINE = { "clax.sidebar": '{"view":"timeline"}' };

/** An artifact with `n` threads on its heading, published by a claude session. */
async function fresh(title: string, n: number) {
  const s = await registerSession(d.base, d.token, "claude", `timeline-${title}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE });
  const ids: string[] = [];
  for (let i = 0; i < n; i++) ids.push((await postThread(d.base, artifact.id, `Comment ${i + 1} on the goals`)).id);
  return { sid: s.id, aid: artifact.id, ids };
}

const box = async (l: Locator) => (await l.boundingBox())!;
const row = (page: Page, id: string) => page.locator(`.tl-row[data-row="${id}"]`);
const tab = (page: Page, name: "Timeline" | "Threads") => page.getByRole("tab", { name });
/** The text of the line thread `id`'s row is listed under: the nearest
 * line before it in the timeline's one list, past the other rows of that
 * line; null when a day heading comes first. */
const lineOf = (page: Page, id: string) => row(page, id).evaluate(r => {
  let e = r.closest(".tl-thread")?.previousElementSibling ?? null;
  while (e?.classList.contains("tl-thread")) e = e.previousElementSibling;
  return e?.classList.contains("tl-line") ? e.querySelector(".tl-text")?.textContent ?? null : null;
});

{
  test(`the sidebar opens on the Timeline, each thread a collapsed row under its line`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { aid, ids } = await fresh(`Opens`, 1);
    await openArtifact(page, d.base, aid, 1);
    await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
    await expect(tab(page, "Threads")).toHaveAttribute("aria-selected", "false");
    await expect(page.locator(".timeline .tl-line").filter({ hasText: "v1 · published by claude" })).toHaveCount(1);
    await expect.poll(() => lineOf(page, ids[0])).toMatch(/sent to no one$/);
    await expect(row(page, ids[0])).toHaveAttribute("aria-expanded", "false");
    await expect(row(page, ids[0]).locator(".tl-st")).toHaveText("Send");
    // The rows stand for the cards, which open on demand; the list's
    // group headings are a tab away.
    await expect(page.locator(".thread-card")).toHaveCount(0);
    await expect(page.locator(".section-open")).toHaveCount(0);
    await expect(page.locator(".sb-tabs .tl-needs")).toHaveText("1 needs you");
  });

  test(`a whole-artifact comment posted above the Timeline closes its composer, and its thread is a row`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { aid } = await fresh(`Posts`, 0);
    await openArtifact(page, d.base, aid, 1);
    await page.locator(".new-comment-open").click();
    const composer = page.locator(".new-comment .composer");
    await expect(composer.locator("textarea")).toBeFocused();
    await page.keyboard.type("Make the colours warmer.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(composer).toHaveCount(0);
    const [t] = (await api(d.base, d.token, `/api/artifacts/${aid}/threads`)).threads as { id: string }[];
    await expect(row(page, t.id).locator(".tl-st")).toHaveText("Send");
    await expect(row(page, t.id).locator(".tl-q")).toHaveText("Make the colours warmer.");
  });

  test(`a thread sent and addressed by v2 is under the v2 line, and its card takes a reply`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { sid, aid, ids: [tid] } = await fresh(`Addressed`, 1);
    await openArtifact(page, d.base, aid, 1);
    // Sent from its card, opened from its row.
    await row(page, tid).click();
    await expect(row(page, tid)).toHaveAttribute("aria-expanded", "true");
    const card = page.locator(`.tl-thread .thread-card[data-thread="${tid}"]`);
    const sent = page.waitForResponse(r => r.url().endsWith(`/threads/${tid}/send`));
    await card.getByRole("button", { name: /^Send to / }).click();
    expect((await sent).ok()).toBe(true);
    await expect(row(page, tid).locator(".tl-st")).toHaveText("sent");
    await publishNext(d.base, d.token, sid, aid, 1, { note: "Two columns", addresses: [tid] });
    await openArtifact(page, d.base, aid, 2);
    await expect(row(page, tid).locator(".tl-st")).toHaveText("✓ v2");
    await expect.poll(() => lineOf(page, tid)).toBe("v2 · Two columns");
    // Listed once, under its newest line.
    await expect(page.locator(`.timeline .tl-row[data-row="${tid}"]`)).toHaveCount(1);
    await expect(page.locator(".sb-tabs .tl-needs")).toHaveCount(0);
    await row(page, tid).click();
    // Reply posts on a key only once a pointer has pressed in the shell.
    const reply = card.getByRole("textbox", { name: "Reply" });
    await reach(page, reply);
    await reply.click();
    await page.keyboard.type("Not quite: the second column is too narrow");
    await page.keyboard.press("Enter");
    await expect(card).toContainText("Not quite: the second column is too narrow");
    await expect.poll(async () => (await api(d.base, d.token, `/api/artifacts/${aid}/threads`)).threads
      .find((t: { id: string }) => t.id === tid)?.comments.length).toBe(2);
    // A person's reply after the version puts the thread back with its agent.
    await expect(row(page, tid).locator(".tl-st")).toHaveText("sent");
  });

  test(`an open card whose thread moves to a newer line stays open, keeping its draft and focus`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { sid, aid, ids: [tid] } = await fresh(`Moves`, 1);
    await openArtifact(page, d.base, aid, 1);
    await streamLive(page);
    await row(page, tid).click();
    const card = page.locator(`.tl-thread .thread-card[data-thread="${tid}"]`);
    const sent = page.waitForResponse(r => r.url().endsWith(`/threads/${tid}/send`));
    await card.getByRole("button", { name: /^Send to / }).click();
    expect((await sent).ok()).toBe(true);
    await expect(row(page, tid).locator(".tl-st")).toHaveText("sent");
    const reply = card.getByRole("textbox", { name: "Reply" });
    await reach(page, reply);
    await reply.click();
    await page.keyboard.type("Half a thought");
    // Marks the card's element, so a remount would show as a card without it.
    await card.evaluate(e => { e.dataset.e2eKept = "1"; });
    await expect.poll(() => lineOf(page, tid)).toBe("Viewer commented");
    // The agent answers from outside the page: a newer line, "claude replied".
    await api(d.base, d.token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body: "Which half?", author_kind: "agent" }) });
    await expect.poll(() => lineOf(page, tid)).toMatch(/^claude(-\d+)? replied$/);
    await expect(row(page, tid).locator(".tl-st")).toHaveText("Reply");
    await expect(card).toContainText("Which half?");
    await expect(card).toHaveAttribute("data-e2e-kept", "1");
    await expect(row(page, tid)).toHaveAttribute("aria-expanded", "true");
    await expect(reply).toHaveValue("Half a thought");
    await expect(reply).toBeFocused();
  });

  test(`the Threads tab shows the thread list, and a reload keeps it`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { aid, ids } = await fresh(`Tabs`, 2);
    await openArtifact(page, d.base, aid, 1);
    await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
    await tab(page, "Threads").click();
    await expect(tab(page, "Threads")).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".timeline")).toHaveCount(0);
    for (const id of ids) await expect(page.locator(`.thread-card[data-thread="${id}"]`)).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem("clax.sidebar"))).toBe('{"view":"threads"}');
    await page.reload();
    await expect(tab(page, "Threads")).toHaveAttribute("aria-selected", "true");
    for (const id of ids) await expect(page.locator(`.thread-card[data-thread="${id}"]`)).toBeVisible();
    // The arrow keys move between the tabs, each choosing its view.
    await tab(page, "Threads").focus();
    await page.keyboard.press("ArrowLeft");
    await expect(tab(page, "Timeline")).toBeFocused();
    await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".timeline")).toBeVisible();
  });

  test(`hidden while docked right, the threads leave the strip on the right, which shows them again`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { aid } = await fresh(`Strip`, 1);
    await openArtifact(page, d.base, aid, 1);
    await page.getByRole("button", { name: "Hide threads" }).click();
    await expect(page.locator("div.drawer")).toHaveCount(0);
    const strip = page.locator("button.rail");
    await expect(strip).toHaveAccessibleName("Threads: 1 pending, 1 needs you. Show threads");
    const r = await box(page.locator("aside.rail"));
    const width = page.viewportSize()!.width;
    expect(r.width).toBe(56);
    expect(r.x + r.width).toBe(width);
    expect((await box(page.locator(".stage"))).width).toBe(width - 56);
    // From the keyboard: the drawer it opens takes the focus, on its Hide threads.
    await strip.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("div.drawer")).toBeVisible();
    await expect(page.locator("aside.rail")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Hide threads" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(strip).toBeVisible();
    // And by a click.
    await strip.click();
    await expect(page.locator("div.drawer")).toBeVisible();
    await expect(tab(page, "Timeline")).toHaveAttribute("aria-selected", "true");
  });

  test(`hidden while docked left, the strip is at the viewer's left`, async ({ page }) => {
    await prefer(page, { ...TIMELINE, "clax.drawer": '{"layout":"left"}' });
    const { aid } = await fresh(`Left`, 2);
    await openArtifact(page, d.base, aid, 1);
    await expect(page.locator("div.drawer")).toBeVisible();
    expect((await box(page.locator("div.drawer"))).x).toBe(0);
    await page.getByRole("button", { name: "Hide threads" }).click();
    const strip = page.locator("button.rail");
    await expect(strip).toHaveAccessibleName("Threads: 2 pending, 2 need you. Show threads");
    const r = await box(page.locator("aside.rail"));
    expect(r.x).toBe(0);
    expect(r.width).toBe(56);
    const stage = await box(page.locator(".stage"));
    expect(stage.x).toBe(56);
    expect(stage.width).toBe(page.viewportSize()!.width - 56);
    await strip.click();
    expect((await box(page.locator("div.drawer"))).x).toBe(0);
    // Narrower than 900 px the threads start hidden: the strip is there
    // from the load, at the left, and the page beside it.
    await page.setViewportSize({ width: 800, height: page.viewportSize()!.height });
    await page.reload();
    await expect(strip).toBeVisible();
    expect(await box(page.locator("aside.rail"))).toMatchObject({ x: 0, width: 56 });
    expect((await box(page.locator(".stage"))).x).toBe(56);
  });

  test(`an artifact with no threads leaves no strip, and the page its whole width`, async ({ page }) => {
    await prefer(page, TIMELINE);
    const { aid } = await fresh(`Empty`, 0);
    await openArtifact(page, d.base, aid, 1);
    await expect(page.locator(".timeline .tl-line").filter({ hasText: "v1 · published by claude" })).toHaveCount(1);
    await page.getByRole("button", { name: "Hide threads" }).click();
    await expect(page.locator("div.drawer")).toHaveCount(0);
    await expect(page.locator("aside.rail")).toHaveCount(0);
    expect((await box(page.locator(".stage"))).width).toBe(page.viewportSize()!.width);
  });
}

/** A port no one listens on now, for a hosted daemon whose public origin names it. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

// The Comments switch lives on a hosted Clax (crates/clax-server/src/hosted.rs).
test(`with Comments off, the Timeline says why its cards have no reply box`, async ({ browser }) => {
  const SECRET = "e2e setup secret";
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const hd = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(hd.port !== port, `the daemon took port ${hd.port}, not ${port}`);
  try {
    const owner = await (await browser.newContext()).newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const { artifact } = await publish(hd.base, hd.token, "Quiet", { "index.html": `<title>Quiet</title>${PAGE}` });
    const aid = artifact.id;
    const made = await owner.evaluate(async id => {
      const f = new FormData();
      f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      f.set("body", "Before the switch"); f.set("version", "1");
      return (await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: f })).status;
    }, aid);
    expect(made).toBe(201);
    const off = await owner.evaluate(async id => (await fetch(`/api/hosted/artifacts/${id}/comments`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ on: false }) })).status, aid);
    expect(off).toBe(200);
    await prefer(owner, TIMELINE);
    await owner.goto(`${origin}/a/${aid}`);
    await expect(tab(owner, "Timeline")).toHaveAttribute("aria-selected", "true");
    const note = owner.locator('[role="tabpanel"] > .no-reply');
    await expect(note).toHaveText("Comments are off for this page.");
    await expect(owner.locator('[role="tabpanel"] > .no-reply + .timeline')).toHaveCount(1);
    const [t] = (await api(hd.base, hd.token, `/api/artifacts/${aid}/threads`)).threads as { id: string }[];
    await row(owner, t.id).click();
    await expect(owner.locator(`.tl-thread .thread-card[data-thread="${t.id}"]`)).toBeVisible();
    await expect(owner.getByRole("textbox", { name: "Reply" })).toHaveCount(0);
  } finally {
    await hd.stop();
  }
});
