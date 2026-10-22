// The owner's agent questions in the app's view (`/a/<id>?app=1`): a question an agent asks
// over the session route shows in the questions sheet the app opens
// (`openQuestions`), the host tells the app how many are open and whether the
// sheet shows (`questions {open, showing}`), the app's dictation fills the
// card's focused text field (`setDraftText`), and answering or skipping
// counts the question closed. The app's side is ./app-host.ts.
import type { Page } from "@playwright/test";
import { host, hostChannel, initHost, ofType } from "./app-host";
import { test, expect, type Daemon, contentFrame, publishAs, registerSession } from "./fixtures";
import { ask, waitAnswer } from "./questions-helpers";
import { advance } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2><p>Grow revenue in every region.</p></main>" };

/** An app made by an app session, and its view in the app with `init` called. */
async function appWith(page: Page, title: string) {
  const s = await registerSession(d.base, d.token, "app");
  const { artifact } = await publishAs(d.base, d.token, s.id, title, PAGE);
  return { sid: s.id, aid: artifact.id, open: async () => {
    await hostChannel(page);
    await page.goto(`${d.base}/a/${artifact.id}?app=1`);
    await contentFrame(page, artifact.id, 1);
    await initHost(page);
  } };
}

/** The latest `questions` message, without its type. */
const questions = async (page: Page) => {
  const m = (await ofType(page, "questions")).at(-1);
  return m ? { open: m.open, showing: m.showing } : null;
};

test("a question asked before load is counted, shown by openQuestions with its ID, filled by dictation and answered", async ({ page }) => {
  const app = await appWith(page, "Questions before load");
  const { question } = await ask(d.base, d.token, app.sid, { source: "ask", artifact_id: app.aid, questions: [{ question: "What should change?", header: "Change", options: [{ label: "Colours" }, { label: "Layout" }], other: true }] });
  await app.open();
  const sheet = page.locator(".app-questions");
  await expect.poll(() => questions(page)).toEqual({ open: 1, showing: false });
  await expect(sheet).toBeHidden();

  await host(page, `openQuestions({ question: ${JSON.stringify(question.id)} })`);
  const card = sheet.locator(`.qcard[data-question="${question.id}"]`);
  await expect(sheet).toBeVisible();
  await expect(card).toBeFocused();
  await expect.poll(() => questions(page)).toEqual({ open: 1, showing: true });
  // The app's agent is Clax: no session name, no project folder, and the button reads "Answer".
  await expect(card.locator(".asker")).toHaveText("Clax");
  await expect(card.locator(".proj")).toHaveCount(0);
  await expect(card.locator("button.primary")).toHaveText("Answer");

  // The app's dictation goes to the card's focused Other field, not to a composer.
  const otherField = card.locator(".opt.other input");
  await otherField.focus();
  await host(page, `setDraftText(${JSON.stringify("Bigger fonts")})`);
  await expect(otherField).toHaveValue("Bigger fonts");
  await card.locator("button.primary").click();

  const answered = await waitAnswer(d.base, d.token, app.sid, question.id);
  expect(answered.status).toBe("answered");
  expect(answered.answers).toEqual([{ selected: [], text: "Bigger fonts" }]);
  await expect.poll(() => questions(page)).toEqual({ open: 0, showing: true });
  // Once the closed card leaves, the sheet closes.
  await advance(page, 4000);
  await expect.poll(() => questions(page)).toEqual({ open: 0, showing: false });
  await expect(sheet).toBeHidden();
});

test("a question asked while the view is open is counted, closeQuestions hides it, and Skip closes it", async ({ page }) => {
  const app = await appWith(page, "Questions after load");
  await app.open();
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  const { question } = await ask(d.base, d.token, app.sid, { source: "ask", artifact_id: app.aid, questions: [{ question: "Ship it?", header: "Ship", options: [{ label: "Yes" }, { label: "No" }] }] });
  await expect.poll(() => questions(page), { timeout: 15_000 }).toEqual({ open: 1, showing: false });

  const sheet = page.locator(".app-questions");
  await host(page, "openQuestions()");
  await expect(sheet).toBeVisible();
  await expect.poll(() => questions(page)).toEqual({ open: 1, showing: true });
  await host(page, "closeQuestions()");
  await expect(sheet).toBeHidden();
  await expect.poll(() => questions(page)).toEqual({ open: 1, showing: false });

  await host(page, "openQuestions()");
  await sheet.locator(`.qcard[data-question="${question.id}"]`).getByRole("button", { name: "Skip" }).click();
  expect((await waitAnswer(d.base, d.token, app.sid, question.id)).status).toBe("declined");
  await expect.poll(() => questions(page)).toMatchObject({ open: 0 });
});
