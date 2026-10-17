// Where a question shows (spec 2026-10-16-ask-where-attention-is-design
// §2, §3), against the real daemon, at 1280 and 390 wide, light and dark:
// a thread's question inside its thread card, counted in the sidebar and
// the Inbox; the sidebar's link opening a folded card; an in-terminal
// question's terminal card with Ask me here, and what it says once
// pressed; the terminal card in /inbox; `?thread=` opening its thread.
// Each state is photographed, to CLAX_E2E_SHOTS when it is set (an
// absolute directory), else to the test's output folder.
import { test, expect, api, openArtifact, publishAs, registerSession } from "./fixtures";
import { ask, threadAsOwner } from "./questions-helpers";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { settle } from "./time";

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2><p>Revenue, hiring and the roadmap.</p></main>" };
const TWO_PAGES = { ...PAGE, "about.html": "<main><h2>Our team</h2></main>" };
const SIZES = [{ width: 1280, height: 860 }, { width: 390, height: 844 }] as const;
const THEMES = ["light", "dark"] as const;
type Size = (typeof SIZES)[number];
type Theme = (typeof THEMES)[number];
/** The session's name, which every card names it by. */
const NAME = "reviewer";

const phone = (size: Size) => size.width < 600;

async function setUp(page: Page, size: Size, theme: Theme) {
  await page.setViewportSize(size);
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
}

/** A session named `NAME`. */
async function namedSession(base: string, token: string) {
  const s = await registerSession(base, token);
  await api(base, token, `/api/sessions/${s.id}`, { method: "PATCH", body: JSON.stringify({ name: NAME }) });
  return s;
}

/** Opens the threads panel: the Threads tab on a phone, else the top bar's toggle. */
async function showThreads(page: Page, size: Size) {
  const toggle = phone(size) ? page.locator(".phone-tabs button", { hasText: "Threads" }) : page.locator("button.threads");
  if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
}

/** Photographs the page as `<state>-<width>-<theme>.png` and checks that it does not scroll sideways. */
async function shot(page: Page, state: string, size: Size, theme: Theme) {
  await page.evaluate(() => document.fonts.ready);
  await settle(page);
  const dir = process.env.CLAX_E2E_SHOTS;
  const name = `${state}-${size.width}-${theme}.png`;
  if (dir) mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: dir ? join(dir, name) : test.info().outputPath(name) });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.width);
}

const inboxCount = (page: Page, size: Size) => page.locator(phone(size) ? ".phone-tabs .inbox-link .count" : ".topbar .inbox-link .count");

for (const theme of THEMES) {
  for (const size of SIZES) {
    test(`a thread's question renders in its card and is counted in the sidebar and the Inbox (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
      const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [{ question: "How big should the title be?", header: "Size", options: [{ label: "Big" }, { label: "Bigger" }] }] });
      expect(q.question.status).toBe("open");
      await openArtifact(page, d.base, artifact.id, 1, "sandbox");
      await showThreads(page, size);
      const card = page.locator(`.thread-card[data-thread="${tid}"]`);
      const inCard = card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`);
      await expect(inCard).toBeVisible();
      await expect(inCard.getByRole("button", { name: /^Answer/ })).toBeVisible();
      // Beneath the thread's comment, above its Reply box.
      const below = await card.evaluate((c, qid) => {
        const box = c.querySelector(`.qcard[data-question="${qid}"]`)!.getBoundingClientRect();
        const last = [...c.querySelectorAll(".msg")].pop()!.getBoundingClientRect();
        const reply = c.querySelector("form.reply")!.getBoundingClientRect();
        return { afterComment: box.top >= last.bottom, beforeReply: box.bottom <= reply.top };
      }, q.question.id);
      expect(below).toEqual({ afterComment: true, beforeReply: true });
      // The sidebar counts it and links to it; it shows there as no card.
      await expect(page.locator(".side-q .gh .c")).toHaveText("1");
      const link = page.locator(".side-q a.q-thread");
      await expect(link).toHaveAttribute("href", `?thread=${tid}`);
      await expect(link).toHaveText(`${NAME} asks in a thread: Size`);
      await expect(page.locator(".side-q > .qlist .qcard")).toHaveCount(0);
      await expect(inboxCount(page, size)).toHaveText(/^[1-9]/);
      await shot(page, "thread-question", size, theme);
    });

    test(`an in-terminal question shows a terminal card with Ask me here, which says it was sent once pressed (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Which chart reads best?");
      const n = await ask(d.base, d.token, s.id, { source: "ask", in_terminal: true, artifact_id: artifact.id, questions: [
        { question: "Which layout?", header: "Layout", options: [{ label: "Two" }, { label: "One" }] },
        { question: "Which palette?", header: "Palette", options: [{ label: "Warm" }, { label: "Cool" }] },
        { question: "Anything else?", header: "Notes" }] });
      const t = await ask(d.base, d.token, s.id, { source: "ask", in_terminal: true, artifact_id: artifact.id, thread_id: tid, questions: [{ question: "Bars or lines?", header: "Chart", options: [{ label: "Bars" }, { label: "Lines" }] }] });
      await openArtifact(page, d.base, artifact.id, 1, "sandbox");
      await showThreads(page, size);
      const side = page.locator(`.side-q .qcard.in-terminal[data-question="${n.question.id}"]`);
      await expect(side.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(side.locator(".hdrs li")).toHaveText(["Layout", "Palette", "Notes"]);
      await expect(side.locator("input, textarea")).toHaveCount(0);
      const pull = side.getByRole("button", { name: "Ask me here" });
      await expect(pull).toBeVisible();
      // The one asked in a thread is a terminal card in that thread's card, and a link in the sidebar.
      const inCard = page.locator(`.thread-card[data-thread="${tid}"] .thread-q-slot .qcard.in-terminal[data-question="${t.question.id}"]`);
      await expect(inCard.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(inCard.getByRole("button", { name: "Ask me here" })).toBeVisible();
      await expect(page.locator(".side-q a.q-thread")).toHaveText(`${NAME} is asking you in the terminal: Chart`);
      await shot(page, "terminal-card", size, theme);
      await pull.click();
      await expect(side.locator(".closed")).toHaveText(`Sent to ${NAME}: it asks here next`);
      await expect(side).toHaveAttribute("data-closed", "");
      const pulled = await api(d.base, d.token, `/api/sessions/${s.id}/questions/${n.question.id}`) as { question: { status: string; pulled?: boolean } };
      expect(pulled.question).toMatchObject({ status: "withdrawn", pulled: true });
      await shot(page, "terminal-pulled", size, theme);
    });

    test(`/inbox shows an in-terminal question as a terminal card (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      // A mirrored (Claude Code hook) question, with no Clax tab open: the daemon asks it in the terminal.
      const n = await ask(d.base, d.token, s.id, { source: "hook", artifact_id: artifact.id, questions: [
        { question: "Which layout?", header: "Layout", options: [{ label: "Two" }, { label: "One" }] },
        { question: "Ship it today?", header: "Ship", options: [{ label: "Yes" }, { label: "No" }] }] });
      expect(n.mode).toBe("terminal");
      await page.goto(`${d.base}/inbox`);
      const card = page.locator(`.inbox .qcard.in-terminal[data-question="${n.question.id}"]`);
      await expect(card.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(card.locator(".about")).toHaveText("Quarterly Review");
      await expect(card.locator(".esc")).toHaveText("After Ask me here, the terminal's dialog stays open until you press Esc there.");
      await expect(card.getByRole("button", { name: "Ask me here" })).toBeVisible();
      await expect(page.locator(".gbar .inbox-link .count")).toHaveText(/^[1-9]/);
      await shot(page, "inbox-terminal", size, theme);
    });
  }
}

for (const size of SIZES) {
  test(`the sidebar's link to a question in a thread of another page opens its folded card and focuses the question (${size.width})`, async ({ page, freshDaemon: d }) => {
    await setUp(page, size, "light");
    const s = await namedSession(d.base, d.token);
    const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", TWO_PAGES);
    const tid = await threadAsOwner(d.base, d.token, artifact.id, "Name the team?", "about.html");
    const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [{ question: "Which name?", header: "Team", options: [{ label: "Makers" }, { label: "Crew" }] }] });
    await openArtifact(page, d.base, artifact.id, 1, "sandbox");
    await showThreads(page, size);
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await expect(card).toHaveClass(/\bfolded\b/);
    await page.locator(".side-q a.q-thread").click();
    await expect(card).not.toHaveClass(/\bfolded\b/);
    const inCard = card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`);
    await expect(inCard).toBeFocused();
    await expect(inCard).toBeInViewport();
    await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}$`));
  });

  test(`?thread= opens the thread it names (${size.width})`, async ({ page, freshDaemon: d }) => {
    await setUp(page, size, "light");
    const s = await namedSession(d.base, d.token);
    const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
    const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
    const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [{ question: "How big?", header: "Size", options: [{ label: "Big" }, { label: "Bigger" }] }] });
    await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
    await page.goto(`${d.base}/a/${artifact.id}?thread=${tid}`);
    await expect(page.locator("aside.sidebar")).toBeVisible();
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await expect(card).toHaveClass(/\bselected\b/);
    await expect(card).toBeInViewport();
    await expect(card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`)).toBeVisible();
  });
}
