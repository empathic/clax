// The question and inbox surfaces against the real daemon: the gallery's
// unread summary, `/inbox`, and a question card that closes and then leaves
// (spec 2026-10-06-agent-questions-and-inbox §9).
import { test, expect, api, publishAs, registerSession } from "./fixtures";
import { advance } from "./time";

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2></main>" };

async function ask(base: string, token: string, sid: string, aid: string, header: string) {
  const r = await api(base, token, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify({
    source: "ask", artifact_id: aid, questions: [{ question: `${header}?`, header, multi_select: false, other: false, options: [{ label: "Yes" }, { label: "No" }] }],
  }) });
  return (r as { question: { id: string } }).question.id;
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
