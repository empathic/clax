// The question and inbox surfaces against the real daemon: the gallery's
// unread summary, `/inbox`, the artifact sidebar, a question card that closes
// and then leaves, notifications, and what a LAN viewer does not see (spec
// 2026-10-06-agent-questions-and-inbox §9).
import { test, expect, api, openArtifact, publishAs, registerSession } from "./fixtures";
import { ask as askAs, waitAnswer } from "./questions-helpers";
import { advance } from "./time";

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2></main>" };

async function ask(base: string, token: string, sid: string, aid: string, header: string) {
  const r = await askAs(base, token, sid, { source: "ask", artifact_id: aid, questions: [{ question: `${header}?`, header, options: [{ label: "Yes" }, { label: "No" }] }] });
  return r.question.id;
}

test("focus the person moved while Skip was pending stays where they put it, after the card closes and after it leaves", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
  const qid = await ask(d.base, d.token, s.id, artifact.id, "Units");
  let release!: () => void;
  const held = new Promise<void>(r => { release = r; });
  await page.route(`**/api/questions/${qid}/decline`, async route => { await held; await route.continue(); });
  await page.goto(`${d.base}/`);
  const card = page.locator(`.inbox-sum .qcard[data-question="${qid}"]`);
  await card.getByRole("button", { name: "Skip" }).click();
  const search = page.getByRole("searchbox", { name: "Search artifacts" });
  await search.click();
  release();
  await expect(card.locator(".closed")).toHaveText("Skipped");
  await expect(search).toBeFocused();
  await advance(page, 4000);
  await expect(card).toHaveCount(0);
  await expect(search).toBeFocused();
});

test("a card that closes with focus keeps it, and the block around it takes it when the card leaves", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
  const qid = await ask(d.base, d.token, s.id, artifact.id, "Legend");
  await page.goto(`${d.base}/`);
  const card = page.locator(`.inbox-sum .qcard[data-question="${qid}"]`);
  await card.getByRole("button", { name: "Skip" }).focus();
  await page.keyboard.press("Enter");
  await expect(card.locator(".closed")).toHaveText("Skipped");
  await expect(card).toBeFocused();
  await advance(page, 4000);
  await expect(card).toHaveCount(0);
  // The summary still lists the artifact's published item, and takes focus.
  await expect(page.locator(".inbox-sum")).toBeFocused();
});

test("/inbox lists the unread items, opens one and marks it read, and keeps its search in the URL", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Sales dashboard", PAGE);
  await page.goto(`${d.base}/inbox`);
  const row = page.locator(".inbox .irow", { hasText: "published Sales dashboard" });
  await expect(row).toHaveAttribute("data-unread", "");
  await expect(page.locator(".gbar .inbox-link .count")).toHaveText("1");
  await expect(page).toHaveTitle(/^\(1\) /);
  await page.getByRole("searchbox", { name: "Search the inbox" }).fill("sales");
  await advance(page, 250);
  await expect(page).toHaveURL(/\/inbox\?search=sales$/);
  await expect(row).toHaveCount(1);
  await row.locator(".open").click();
  await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}`));
  const left = await api(d.base, d.token, "/api/inbox/summary") as { unread: number };
  expect(left.unread).toBe(0);
});

test("an ask answered in the gallery reaches the agent", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  await page.goto(`${d.base}/`);
  const pending = askAs(d.base, d.token, s.id, { source: "ask", questions: [
    { question: "Which layout?", header: "Layout", options: [{ label: "Two", preview: "|a|b|" }, { label: "One", preview: "|ab|" }] },
    { question: "Anything else?", header: "Notes" }] });
  const card = page.locator(".inbox-sum .qcard[data-question]");
  await expect(card).toBeVisible();
  await expect(page).toHaveTitle(/^\(\d+\) /);
  await card.getByLabel("One").click();
  await expect(card.locator("pre.preview")).toHaveText("|ab|");
  await card.getByRole("tab", { name: "Notes" }).click();
  await card.locator("textarea").fill("keep it light");
  await card.getByRole("button", { name: "Answer claude" }).click();
  const q = await waitAnswer(d.base, d.token, s.id, (await pending).question.id);
  expect(q.status).toBe("answered");
  expect(q.answered_via).toBe("shell");
  expect(q.answers).toEqual([{ selected: ["One"], text: null }, { selected: [], text: "keep it light" }]);
});

test("a question about an artifact shows above its threads", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Board", PAGE);
  await page.goto(`${d.base}/a/${artifact.id}`);
  await expect(page.locator(".sidebar .section-open, .sidebar .empty-threads").first()).toBeAttached();
  const { question } = await askAs(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, questions: [{ question: "Ship?", header: "Ship", options: [{ label: "Yes" }, { label: "No" }] }] });
  const card = page.locator(`.sidebar .qcard[data-question="${question.id}"]`);
  await expect(card).toBeVisible();
  const above = await page.evaluate(() => {
    const q = document.querySelector(".sidebar [data-question]")!;
    const t = document.querySelector(".sidebar .section-open, .sidebar .empty-threads");
    return !t || !!(q.compareDocumentPosition(t) & Node.DOCUMENT_POSITION_FOLLOWING);
  });
  expect(above).toBe(true);
});

test("a LAN viewer sees no questions and no inbox, and asks for neither", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Board", PAGE);
  await askAs(d.base, d.token, s.id, { source: "ask", questions: [{ question: "Q", header: "H" }] });
  const asked: string[] = [];
  page.on("request", r => { const u = new URL(r.url()); if (/^\/api\/(questions|inbox)/.test(u.pathname)) asked.push(u.pathname); });
  await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(page.locator(".sidebar")).toBeVisible();
  await expect(page.locator(".sidebar .qcard, .inbox-link")).toHaveCount(0);
  await page.goto(`${d.base}/`);
  await expect(page.locator(".card", { hasText: "Board" }).first()).toBeVisible();
  await expect(page.locator(".inbox-sum .qcard, .inbox-sum .irow, .inbox-link")).toHaveCount(0);
  expect(asked).toEqual([]);
});

test("a notification fires when no Clax tab has focus", async ({ page, freshDaemon: d }) => {
  await page.addInitScript(() => {
    const shown: string[] = [];
    const w = window as unknown as { claxShown: string[]; Notification: unknown };
    w.claxShown = shown;
    class Fake {
      static permission = "granted";
      static async requestPermission() { return "granted"; }
      tag: string;
      onclick: unknown = null;
      constructor(title: string, o: { tag?: string } = {}) { shown.push(title); this.tag = o.tag ?? ""; }
      close() {}
    }
    w.Notification = Fake;
    // No Clax tab has focus: this one says it lost it.
    Document.prototype.hasFocus = () => false;
  });
  // The feeds subscribe first, then fetch: once the questions are fetched, the stream holds `inbox`.
  const fed = page.waitForResponse(r => new URL(r.url()).pathname === "/api/questions");
  await page.goto(`${d.base}/`);
  await fed;
  await page.evaluate(() => { window.dispatchEvent(new Event("blur")); });
  const s = await registerSession(d.base, d.token);
  await askAs(d.base, d.token, s.id, { source: "ask", questions: [{ question: "Q", header: "Pick", options: [{ label: "A" }, { label: "B" }] }] });
  await expect.poll(() => page.evaluate(() => (window as unknown as { claxShown: string[] }).claxShown)).toEqual(["claude asks: Pick"]);
});
