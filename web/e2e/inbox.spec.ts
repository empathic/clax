// The inbox end to end against the real daemon (spec
// 2026-10-06-agent-questions-and-inbox §7, §8, §9.2): an agent's reply arrives
// unread, opening it marks it read, the read history stays searchable, and
// Mark all read empties the count.
import { test, expect, publishAs, registerSession } from "./fixtures";
import { replyAsAgent, threadAsOwner } from "./questions-helpers";

const PAGE = { "index.html": "<main><h2>Board</h2></main>" };

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
