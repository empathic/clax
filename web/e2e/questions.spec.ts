// The question and inbox surfaces against the real daemon: the gallery's
// unread summary, `/inbox`, the artifact sidebar, a question card that closes
// and then leaves, notifications, and what a LAN viewer does not see (spec
// 2026-10-06-agent-questions-and-inbox §9).
import { test, expect, api, openArtifact, publishAs, registerSession } from "./fixtures";
import { ask as askAs, waitAnswer } from "./questions-helpers";
import type { Locator, Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { advance, settle } from "./time";

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
  // Its row, under the page's heading.
  const group = page.locator(`.inbox .igroup[data-group="${artifact.id}"]`);
  await expect(group.locator(".gtitle")).toHaveText("Sales dashboard, 1 new");
  const row = group.locator(".irow", { hasText: "published the page" });
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
  await card.getByLabel("One").hover();
  await expect(card.locator("pre.preview")).toHaveText("|ab|");
  // Picking moves on to the unanswered Notes.
  await card.getByLabel("One").click();
  await advance(page, 250);
  await expect(card.getByRole("tab", { name: "Notes" })).toHaveAttribute("aria-selected", "true");
  await card.locator("textarea").fill("keep it light");
  await card.getByRole("button", { name: "Answer claude" }).click();
  const q = await waitAnswer(d.base, d.token, s.id, (await pending).question.id);
  expect(q.status).toBe("answered");
  expect(q.answered_via).toBe("shell");
  expect(q.answers).toEqual([{ selected: ["One"], text: null }, { selected: [], text: "keep it light" }]);
});

test("an agent's question renders as Markdown in its card, its options' text and preview as written", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
  const question = "Which **layout** should the review use? It sets `grid-template-columns` in [layout.css](https://example.com/layout.css).\n\n- charts first\n- <b>no</b> raw HTML";
  const preview = "┌──────┬──────┐\n│ **a** │ `b`  │\n└──────┴──────┘";
  await askAs(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, questions: [{ question, header: "Layout",
    options: [{ label: "Two columns", description: "Charts left, *table* right", preview }, { label: "One column", description: "Everything stacked" }] }] });
  await page.goto(`${d.base}/inbox`);
  const card = page.locator(".inbox .qcard[data-question]");
  const text = card.locator(".qtext .md");
  await expect(text.locator("strong")).toHaveText("layout");
  await expect(text.locator("code")).toHaveText("grid-template-columns");
  await expect(text.locator("a")).toHaveAttribute("href", "https://example.com/layout.css");
  await expect(text.locator("a")).toHaveAttribute("rel", "noopener noreferrer nofollow");
  await expect(text.locator("li")).toHaveText(["charts first", "<b>no</b> raw HTML"]);
  await expect(card.locator("b, img, script")).toHaveCount(0);
  // The options are still named by the question; descriptions and previews stay as written.
  await expect(card.getByRole("radiogroup", { name: /Which layout should the review use\?/ })).toBeVisible();
  await expect(card.locator(".desc").first()).toHaveText("Charts left, *table* right");
  await card.getByLabel("Two columns").hover();
  await expect(card.locator("pre.preview")).toHaveText(preview);
  await expect(card.locator("pre.preview > *")).toHaveCount(0);
  const dir = process.env.CLAX_MD_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of [["desktop", { width: 1440, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
      await page.setViewportSize(viewport);
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await card.getByLabel("Two columns").hover();
      await card.scrollIntoViewIfNeeded();
      await settle(page);
      await card.screenshot({ path: join(dir, `${theme}-${size}-question.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
    }
  }
});

test("a question about an artifact shows above its threads, and another page's thread opens in place below it", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Board", { ...PAGE, "about.html": "<main><h2>Our team</h2></main>" });
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Our team", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "about.html" }));
  form.set("body", "Name the team.");
  form.set("version", "1");
  const made = await fetch(`${d.base}/api/artifacts/${artifact.id}/threads`, { method: "POST", body: form });
  expect(made.status).toBe(201);
  const tid = (await made.json()).thread.id as string;
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
  // The thread on about.html opens in place, the question still above it and the frame on its page.
  const head = page.locator(`.sidebar [data-thread="${tid}"] button.card-head`);
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await head.click();
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator(`.sidebar [data-thread="${tid}"] .msg .body`)).toHaveText(["Name the team."]);
  await expect(card).toBeVisible();
  expect(await card.evaluate((q, id) => !!(q.compareDocumentPosition(document.querySelector(`.sidebar [data-thread="${id}"]`)!) & Node.DOCUMENT_POSITION_FOLLOWING), tid)).toBe(true);
  await expect(page).toHaveURL(`${d.base}/a/${artifact.id}`);
  const shots = process.env.CLAX_E2E_SHOTS;
  if (shots) await page.screenshot({ path: `${shots}/shell-sidebar-question-and-in-place.png` });
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

/** Whether the daemon counts an owner surface open for mirrored questions (a debug build's). */
async function surfaceOpen(base: string, token: string): Promise<boolean> {
  return ((await api(base, token, "/api/_test/stream/open")) as { owner_surface: boolean }).owner_surface;
}

/** Where CLAX_QUESTION_SHOTS=1 saves this file's screenshots. */
const SHOTS = fileURLToPath(new URL("../../target/question-shots/", import.meta.url));

/** With CLAX_QUESTION_SHOTS=1, photographs `card` (and with `whole`, the page) at 1280 and 390 px wide, light and dark, as `<theme>-<width>-<name>`. */
async function shoot(page: Page, card: Locator, name: string, whole: boolean) {
  if (process.env.CLAX_QUESTION_SHOTS !== "1") return;
  for (const [width, height] of [[1280, 900], [390, 844]]) {
    for (const theme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await card.scrollIntoViewIfNeeded();
      await settle(page);
      if (whole) await page.screenshot({ path: `${SHOTS}${theme}-${width}-${name}.png` });
      await card.screenshot({ path: `${SHOTS}${theme}-${width}-${name}-card.png` });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.emulateMedia({ colorScheme: "light" });
}

test("a mirrored question of four parts moves on after a pick, moves either way, says what is left, and answers the hook", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Pipeline", PAGE);
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(`${d.base}/`);
  await expect.poll(() => surfaceOpen(d.base, d.token)).toBe(true);
  // AskUserQuestion's input, as the hook sends it.
  const single = (question: string, header: string, labels: string[]) => ({ question, header, multiSelect: false, options: labels.map(label => ({ label, description: `${label}, described` })) });
  const made = await api(d.base, d.token, `/api/sessions/${s.id}/questions`, { method: "POST", body: JSON.stringify({ source: "hook", tool_use_id: "toolu_four", questions: [
    single("Which classifier should label the events?", "Classifier", ["Rules", "Model"]),
    single("Where should the labels be stored?", "Storage", ["SQLite", "Parquet"]),
    { ...single("Which sources should be read?", "Sources", ["Logs", "Traces", "Metrics"]), multiSelect: true },
    single("How often should it run?", "Schedule", ["Hourly", "Daily"]),
  ] }) }) as { mode: string; question: { id: string } };
  expect(made.mode).toBe("wait");
  const qid = made.question.id;
  const card = page.locator(`.inbox-sum .qcard[data-question="${qid}"]`);
  const tabs = card.getByRole("tab");
  const answers = card.locator(".chip .ans");
  const primary = card.locator("button.primary");
  const shown = (i: number) => expect(tabs.nth(i)).toHaveAttribute("aria-selected", "true");
  await expect(primary).toHaveText("Answer 4 more");
  await expect(answers).toHaveText(["Not answered", "Not answered", "Not answered", "Not answered"]);
  await expect(card.locator(".steps .pos")).toHaveText("1 of 4");
  await shoot(page, card, "none-answered", false);

  // A single choice's pick moves on to the next part.
  await card.getByLabel("Rules").click();
  await advance(page, 250);
  await shown(1);
  await expect(card.getByLabel("SQLite")).toBeFocused();
  await expect(tabs.nth(0)).toHaveAttribute("data-done", "");
  await expect(answers.nth(0)).toHaveText("Rules");
  await expect(primary).toHaveText("Answer 3 more");
  await shoot(page, card, "one-answered", true);
  await card.getByLabel("SQLite").focus();

  // Back and Next move either way; from the third part, an early Answer
  // goes to the first unanswered (Storage).
  await card.getByRole("button", { name: "Back" }).click();
  await shown(0);
  await card.getByRole("button", { name: "Next" }).click();
  await card.getByRole("button", { name: "Next" }).click();
  await shown(2);
  await primary.click();
  await shown(1);
  await expect(card.getByLabel("SQLite")).toBeFocused();
  // Keys: 2 picks Parquet and moves on to Sources, a multi choice, which
  // waits; N moves on.
  await page.keyboard.press("2");
  await advance(page, 250);
  await shown(2);
  await card.getByLabel("Logs").click();
  await card.getByLabel("Traces").click();
  await advance(page, 1000);
  await shown(2);
  await page.keyboard.press("n");
  await shown(3);
  await expect(answers).toHaveText(["Rules", "Parquet", "Logs, Traces", "Not answered"]);
  // The last part's Other text needs no move; Answer is then the agent's.
  await card.getByPlaceholder("Type your own answer").fill("Every 15 minutes");
  await expect(primary).toHaveText("Answer claude");
  await expect(primary).not.toHaveAttribute("data-left");
  await shoot(page, card, "all-answered", false);
  if (process.env.CLAX_QUESTION_SHOTS === "1") {
    // The same question in the artifact view's sidebar, as it first shows.
    const view = await page.context().newPage();
    await view.goto(`${d.base}/a/${artifact.id}`);
    await shoot(view, view.locator(`.qcard[data-question="${qid}"]`), "sidebar", true);
    await view.close();
  }
  await primary.click();
  const q = await waitAnswer(d.base, d.token, s.id, qid);
  expect(q.status).toBe("answered");
  expect(q.answers).toEqual([
    { selected: ["Rules"], text: null }, { selected: ["Parquet"], text: null },
    { selected: ["Logs", "Traces"], text: null }, { selected: [], text: "Every 15 minutes" },
  ]);
});
