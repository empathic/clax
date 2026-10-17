// Where a question shows (spec 2026-10-16-ask-where-attention-is-design
// §2, §3), against the real daemon, at 1280 and 390 wide, light and dark:
// a thread's question arriving live inside its thread card, counted in the
// sidebar and the Inbox, and leaving it as it was once answered or skipped;
// a resolved thread's question moving live to the sidebar's block; the sidebar's link
// opening a folded card; an in-terminal question's terminal card with Ask
// me here, what it says once pressed and when sending fails; the terminal
// card in /inbox and the gallery; `?thread=` opening its thread.
//
// With CLAX_E2E_SHOTS set, each state is photographed as
// `<state>-<width>-<theme>.png`: into CLAX_E2E_SHOTS when it is an absolute
// directory, else into target/ask-where/shots/. Under Bazel, which runs the
// tests in a copy of the project, give the absolute directory:
//   bazelisk test //web:e2e_rest --test_arg=--grep=ask-where.spec \
//     --test_env=CLAX_E2E_SHOTS=$PWD/target/ask-where/shots
import { test, expect, api, openArtifact, publishAs, registerSession } from "./fixtures";
import { ask, threadAsOwner } from "./questions-helpers";
import type { Locator, Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";
import { advance, settle } from "./time";

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2><p>Revenue, hiring and the roadmap.</p></main>" };
const TWO_PAGES = { ...PAGE, "about.html": "<main><h2>Our team</h2></main>" };
const SIZES = [{ width: 1280, height: 860 }, { width: 390, height: 844 }] as const;
const THEMES = ["light", "dark"] as const;
type Size = (typeof SIZES)[number];
type Theme = (typeof THEMES)[number];
/** The session's name, which every card names it by. */
const NAME = "reviewer";
const SIZE_Q = { question: "How big should the title be?", header: "Size", options: [{ label: "Big" }, { label: "Bigger" }] };

const SHOTS_ENV = process.env.CLAX_E2E_SHOTS;
const SHOTS = !SHOTS_ENV ? null : isAbsolute(SHOTS_ENV) ? SHOTS_ENV : fileURLToPath(new URL("../../target/ask-where/shots/", import.meta.url));

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
  await expect(toggle).toHaveAttribute("aria-pressed", /^(true|false)$/);
  if ((await toggle.getAttribute("aria-pressed")) === "false") await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("aside.sidebar")).toBeVisible();
}

/** Checks that the page does not scroll sideways, and with CLAX_E2E_SHOTS photographs it as `<state>-<width>-<theme>.png`. */
async function shot(page: Page, state: string, size: Size, theme: Theme) {
  await page.evaluate(() => document.fonts.ready);
  await settle(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.width);
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${state}-${size.width}-${theme}.png`) });
}

const inboxCount = (page: Page, size: Size) => page.locator(phone(size) ? ".phone-tabs .inbox-link .count" : ".topbar .inbox-link .count");

/** Whether a terminal card's time sits on its head's first line, beside
 * who asks, and the head shows all of who asks and where, unclipped and in
 * no frame of its own. */
const timeOnFirstLine = (card: Locator) => card.evaluate(c => {
  const asker = c.querySelector(".asker")!.getBoundingClientRect();
  const time = c.querySelector("time")!.getBoundingClientRect();
  const asking = c.querySelector<HTMLElement>(".qhead > :first-child")!;
  const style = getComputedStyle(asking);
  return time.top < asker.top + 20 && time.left > asker.left && asking.scrollHeight <= asking.clientHeight && style.borderTopWidth === "0px";
});

/** A thread card's box and the top of the next thing after its last comment. */
const cardBox = (card: Locator) => card.evaluate(c => {
  const r = c.getBoundingClientRect();
  const slot = c.querySelector(".thread-q-slot")!.getBoundingClientRect();
  return { top: Math.round(r.top), height: Math.round(r.height), slot: Math.round(slot.height) };
});

for (const theme of THEMES) {
  for (const size of SIZES) {
    test(`a thread's question arrives live in its card, is counted in the sidebar and the Inbox, and once answered leaves the card as it was (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
      await openArtifact(page, d.base, artifact.id, 1, "sandbox");
      await showThreads(page, size);
      const card = page.locator(`.thread-card[data-thread="${tid}"]`);
      await expect(card).toBeVisible();
      // No question: no count, and the card's slot takes no space.
      await expect(inboxCount(page, size)).toHaveCount(0);
      const before = await cardBox(card);
      expect(before.slot).toBe(0);
      const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [SIZE_Q] });
      const inCard = card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`);
      await expect(inCard).toBeVisible();
      await expect(inCard.getByRole("button", { name: `Answer ${NAME}` })).toBeVisible();
      // Beneath the thread's comment, above its Reply box.
      const placed = await card.evaluate((c, qid) => {
        const box = c.querySelector(`.qcard[data-question="${qid}"]`)!.getBoundingClientRect();
        const last = [...c.querySelectorAll(".msg")].pop()!.getBoundingClientRect();
        const reply = c.querySelector("form.reply")!.getBoundingClientRect();
        return { afterComment: box.top >= last.bottom, beforeReply: box.bottom <= reply.top };
      }, q.question.id);
      expect(placed).toEqual({ afterComment: true, beforeReply: true });
      // The sidebar counts it and links to it; it shows there as no card.
      await expect(page.locator(".side-q .gh .c")).toHaveText("1");
      const link = page.locator(".side-q a.q-thread");
      await expect(link).toHaveAttribute("href", `?thread=${tid}`);
      await expect(link).toHaveText(`${NAME} asks in a thread: Size`);
      await expect(page.locator(".side-q .qcard")).toHaveCount(0);
      await expect(inboxCount(page, size)).toHaveText("1");
      await shot(page, "thread-question", size, theme);
      // Answering it neither selects nor scrolls the thread card; the label shows, then it leaves with no gap.
      const top = (await cardBox(card)).top;
      await inCard.getByRole("radio").first().check();
      await inCard.getByRole("button", { name: `Answer ${NAME}` }).click();
      await expect(inCard.locator(".closed")).toHaveText("Answered");
      await expect(card).not.toHaveClass(/\bselected\b/);
      expect((await cardBox(card)).top).toBe(top);
      await expect(inboxCount(page, size)).toHaveCount(0);
      await advance(page, 4000);
      await expect(inCard).toHaveCount(0);
      await expect.poll(() => cardBox(card)).toEqual(before);
    });

    test(`an in-terminal question shows a terminal card with Ask me here, which says it was sent once pressed, or that it was not (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Which chart reads best?");
      await openArtifact(page, d.base, artifact.id, 1, "sandbox");
      await showThreads(page, size);
      await expect(page.locator(`.thread-card[data-thread="${tid}"]`)).toBeVisible();
      await expect(inboxCount(page, size)).toHaveCount(0);
      // Both arrive live: the terminal card loads on demand.
      const n = await ask(d.base, d.token, s.id, { source: "ask", in_terminal: true, artifact_id: artifact.id, questions: [
        { question: "Which layout?", header: "Layout", options: [{ label: "Two" }, { label: "One" }] },
        { question: "Which palette?", header: "Palette", options: [{ label: "Warm" }, { label: "Cool" }] },
        { question: "Anything else?", header: "Notes" }] });
      const t = await ask(d.base, d.token, s.id, { source: "ask", in_terminal: true, artifact_id: artifact.id, thread_id: tid, questions: [{ question: "Bars or lines?", header: "Chart", options: [{ label: "Bars" }, { label: "Lines" }] }] });
      const side = page.locator(`.side-q .qcard.in-terminal[data-question="${n.question.id}"]`);
      await expect(side.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(side.locator(".hdrs li")).toHaveText(["Layout", "Palette", "Notes"]);
      await expect(side.locator("input, textarea")).toHaveCount(0);
      const pull = side.getByRole("button", { name: "Ask me here" });
      await expect(pull).toBeVisible();
      expect(await timeOnFirstLine(side)).toBe(true);
      // The one asked in a thread is a terminal card in that thread's card, and a link in the sidebar.
      const inCard = page.locator(`.thread-card[data-thread="${tid}"] .thread-q-slot .qcard.in-terminal[data-question="${t.question.id}"]`);
      await expect(inCard.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(inCard.getByRole("button", { name: "Ask me here" })).toBeVisible();
      expect(await timeOnFirstLine(inCard)).toBe(true);
      await expect(page.locator(".side-q a.q-thread")).toHaveText(`${NAME} is asking you in the terminal: Chart`);
      await expect(page.locator(".side-q .gh .c")).toHaveText("2");
      await expect(inboxCount(page, size)).toHaveText("2");
      await shot(page, "terminal-card", size, theme);
      await pull.click();
      await expect(side.locator(".closed")).toHaveText(`Sent to ${NAME}: it asks here when it next checks in`);
      await expect(side).toHaveAttribute("data-closed", "");
      const pulled = await api(d.base, d.token, `/api/sessions/${s.id}/questions/${n.question.id}`) as { question: { status: string; pulled?: boolean } };
      expect(pulled.question).toMatchObject({ status: "withdrawn", pulled: true });
      await expect(inboxCount(page, size)).toHaveText("1");
      // A pull that fails says so, and leaves the question open.
      await page.route(`**/api/questions/${t.question.id}/pull`, r => r.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { code: "unavailable", message: "down" } }) }));
      await inCard.getByRole("button", { name: "Ask me here" }).click();
      await expect(inCard.locator(".err")).toHaveText("Not sent: try again");
      await expect(inCard.getByRole("button", { name: "Ask me here" })).toBeEnabled();
      await expect(inboxCount(page, size)).toHaveText("1");
      await shot(page, "terminal-pulled", size, theme);
    });

    test(`/inbox and the gallery show an in-terminal question as a terminal card (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
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
      expect(await timeOnFirstLine(card)).toBe(true);
      // The question and the published item.
      await expect(page.locator(".gbar .inbox-link .count")).toHaveText("2");
      await shot(page, "inbox-terminal", size, theme);
      await page.goto(`${d.base}/`);
      const sum = page.locator(`.inbox-sum .qcard.in-terminal[data-question="${n.question.id}"]`);
      await expect(sum.locator(".asker")).toHaveText(`${NAME} is asking you in the terminal`);
      await expect(sum.getByRole("button", { name: "Ask me here" })).toBeVisible();
      await shot(page, "gallery-terminal", size, theme);
    });

    test(`the sidebar's link to a question in a thread of another page opens its folded card and focuses the question (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
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
      await shot(page, "folded-link-opened", size, theme);
    });

    test(`?thread= opens the thread it names (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await setUp(page, size, theme);
      const s = await namedSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      // An earlier thread, so the one named is not simply the first.
      await threadAsOwner(d.base, d.token, artifact.id, "Is this the right heading?");
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
      const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [SIZE_Q] });
      await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
      await page.goto(`${d.base}/a/${artifact.id}?thread=${tid}`);
      await expect(page.locator("aside.sidebar")).toBeVisible();
      const card = page.locator(`.thread-card[data-thread="${tid}"]`);
      await expect(card).toHaveClass(/\bselected\b/);
      await expect(card).toBeInViewport();
      await expect(card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`)).toBeVisible();
      await shot(page, "thread-param", size, theme);
    });
  }
}

for (const size of SIZES) {
  test(`skipping a thread's question leaves its card unselected and in place, and the slot collapses (${size.width})`, async ({ page, freshDaemon: d }) => {
    await setUp(page, size, "light");
    const s = await namedSession(d.base, d.token);
    const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
    const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
    await openArtifact(page, d.base, artifact.id, 1, "sandbox");
    await showThreads(page, size);
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await expect(card).toBeVisible();
    const before = await cardBox(card);
    const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [SIZE_Q] });
    const inCard = card.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`);
    await expect(inCard).toBeVisible();
    const top = (await cardBox(card)).top;
    await inCard.getByRole("button", { name: "Skip" }).click();
    await expect(inCard.locator(".closed")).toHaveText("Skipped");
    await expect(card).not.toHaveClass(/\bselected\b/);
    expect((await cardBox(card)).top).toBe(top);
    await advance(page, 4000);
    await expect(inCard).toHaveCount(0);
    await expect.poll(() => cardBox(card)).toEqual(before);
  });

  test(`a resolved thread's question moves to the sidebar's block as a card (${size.width})`, async ({ page, freshDaemon: d }) => {
    await setUp(page, size, "light");
    const s = await namedSession(d.base, d.token);
    const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
    const tid = await threadAsOwner(d.base, d.token, artifact.id, "Make the title bigger?");
    const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [SIZE_Q] });
    await openArtifact(page, d.base, artifact.id, 1, "sandbox");
    await showThreads(page, size);
    await expect(page.locator(`.thread-card[data-thread="${tid}"] .qcard[data-question="${q.question.id}"]`)).toBeVisible();
    await expect(page.locator(".side-q .qcard")).toHaveCount(0);
    await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/resolve`, { method: "POST", session: s.id, body: JSON.stringify({ as: "agent" }) });
    // The daemon sends the question again when its thread resolves: the
    // open page moves it with no reload.
    const sideCard = page.locator(`.side-q .qcard[data-question="${q.question.id}"]`);
    await expect(sideCard).toBeVisible();
    await expect(sideCard.getByRole("button", { name: `Answer ${NAME}` })).toBeVisible();
    await expect(page.locator(".side-q a.q-thread")).toHaveCount(0);
    await expect(page.locator(`.thread-q-slot .qcard[data-question="${q.question.id}"]`)).toHaveCount(0);
  });
}
