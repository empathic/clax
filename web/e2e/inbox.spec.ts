// The inbox end to end against the real daemon (spec
// 2026-10-06-agent-questions-and-inbox §7, §8, §9.2): an agent's reply arrives
// unread, opening it marks it read, the read history stays searchable, and
// Mark all read empties the count.
import { mkdirSync } from "node:fs";
import { test, expect, api, publishAs, registerSession } from "./fixtures";
import { ask, replyAsAgent, threadAsOwner } from "./questions-helpers";

const PAGE = { "index.html": "<main><h2>Board</h2></main>" };
/** A directory to photograph the grouped inbox into, light and dark, desktop and phone; unset, none is taken. */
const SHOTS = process.env.CLAX_INBOX_SHOTS;

test("replies arrive unread, a look marks them read, and history stays searchable", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Board", PAGE);
  const tid = await threadAsOwner(d.base, d.token, artifact.id, "make it blue");
  await replyAsAgent(d.base, d.token, s.id, artifact.id, tid, "Done: the header is blue");
  await page.goto(`${d.base}/inbox`);
  const unread = page.locator('.inbox .sec[aria-labelledby="inbox-unread-h"]');
  const row = unread.locator(".irow", { hasText: "the header is blue" });
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute("data-unread", "");
  // The artifact's publication and the reply.
  await expect(page.locator(".gbar .inbox-link .count")).toHaveText("2");
  // Opens the thread; the look marks the reply read.
  await row.locator(".open").click();
  await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}`));

  await page.goto(`${d.base}/inbox?search=blue`);
  await expect(unread.locator(".note")).toHaveText("No unread items match.");
  await expect(unread.locator(".irow", { hasText: "blue" })).toHaveCount(0);
  await page.getByRole("button", { name: /Show \d+ read item/ }).click();
  await expect(page.locator("#inbox-read .irow", { hasText: "the header is blue" })).toBeVisible();

  // A second reply comes in while the inbox is open; Mark all read reads it.
  await page.goto(`${d.base}/inbox`);
  await replyAsAgent(d.base, d.token, s.id, artifact.id, tid, "Also bolded the title");
  await expect(unread.locator(".irow", { hasText: "Also bolded the title" })).toBeVisible();
  await expect(page.locator(".gbar .inbox-link .count")).not.toHaveText("");
  await page.getByRole("button", { name: "Mark all read" }).click();
  await expect(page.locator(".gbar .inbox-link .count")).toBeHidden();
});

test("groups the inbox by page: counts across pages, folding, a group's Mark read, the Page filter", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const pub = async (title: string) => (await publishAs(d.base, d.token, s.id, title, PAGE)).artifact.id;
  const pricing = await pub("Pricing page");
  // Pricing's publication is read: its group shows in Read alone.
  await api(d.base, d.token, "/api/inbox/read", { method: "POST", body: JSON.stringify({ all: true, filter: { artifact: pricing } }) });
  const onboarding = await pub("Onboarding doc");
  await ask(d.base, d.token, s.id, { source: "ask", questions: [{ question: "Ship it today?", header: "Ship", options: [{ label: "Yes" }, { label: "No" }] }] });
  const sales = await pub("Sales dashboard");
  const tid = await threadAsOwner(d.base, d.token, sales, "make the chart bigger");
  for (const n of [1, 2, 3, 4]) await replyAsAgent(d.base, d.token, s.id, sales, tid, `Reply ${n} on the chart`);

  await page.goto(`${d.base}/inbox`);
  const unread = page.locator('.inbox .sec[aria-labelledby="inbox-unread-h"]');
  const group = (key: string) => unread.locator(`.igroup[data-group="${key}"]`);
  // The question about no page is a card; its group, Other, has nothing else, so it does not show.
  await expect(unread.locator(".qcard")).toHaveCount(1);
  await expect(unread.locator(".pinned-h")).toHaveText("Questions1");
  await expect(unread.locator(".igroup .gtitle")).toHaveText(["Sales dashboard", "Onboarding doc"]);
  // Five new on Sales (its publication and four replies), three listed, the rest a Show more away.
  await expect(group(sales).locator(".gmeta")).toHaveText(/^5 new · /);
  await expect(group(sales).locator(".irow")).toHaveCount(3);
  await expect(group(sales).locator(".irow").first()).toContainText("Reply 4 on the chart");
  await group(sales).getByRole("button", { name: "Show more" }).click();
  await expect(group(sales).locator(".irow")).toHaveCount(5);
  await expect(group(sales).getByRole("button", { name: "Show more" })).toHaveCount(0);
  // The title opens the page; the toggle folds the group.
  await expect(group(sales).locator("a.gtitle")).toHaveAttribute("href", `/a/${sales}`);
  await group(sales).getByRole("button", { name: "Hide Sales dashboard" }).click();
  await expect(group(sales).locator(".irow")).toHaveCount(0);
  await group(sales).getByRole("button", { name: "Show Sales dashboard" }).click();
  await expect(group(sales).locator(".irow")).toHaveCount(5);

  // A reply arrives: its group counts it.
  await replyAsAgent(d.base, d.token, s.id, sales, tid, "Reply 5 on the chart");
  await expect(group(sales).locator(".gmeta")).toHaveText(/^6 new · /);

  if (SHOTS) {
    mkdirSync(SHOTS, { recursive: true });
    for (const [size, viewport] of [["desktop", { width: 1440, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
      await page.setViewportSize(viewport);
      for (const theme of ["light", "dark"] as const) {
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        // Loaded in the theme, so no colour is caught mid-transition.
        await page.reload();
        await expect(group(sales).locator(".irow")).toHaveCount(3);
        await page.screenshot({ path: `${SHOTS}/inbox-${theme}-${size}.png`, fullPage: true });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
      }
    }
  }

  // A group's Mark read reads that group alone; it moves to Read.
  await group(onboarding).getByRole("button", { name: "Mark Onboarding doc read" }).click();
  await expect(group(onboarding)).toHaveCount(0);
  await expect(group(sales).locator(".gmeta")).toHaveText(/^6 new · /);
  // Read folds the same way: its groups folded, newest first.
  await page.getByRole("button", { name: /Show \d+ read items?/ }).click();
  const read = page.locator("#inbox-read");
  await expect(read.locator(".igroup .gtitle")).toHaveText(["Onboarding doc", "Pricing page"]);
  await expect(read.locator(".irow")).toHaveCount(0);
  await read.getByRole("button", { name: "Show Pricing page" }).click();
  await expect(read.locator(`.igroup[data-group="${pricing}"] .irow`)).toHaveCount(1);

  // The Page filter: one group.
  await page.goto(`${d.base}/inbox?artifact=${sales}`);
  await expect(unread.locator(".igroup")).toHaveCount(1);
  await expect(unread.locator(".qcard")).toHaveCount(0);
});
