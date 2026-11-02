// A queue's own page and a report's own page (the DevTools panel BRIEF's
// Addendum 2), on a real daemon: the owner opens /queues/<qid> from the
// Queues index, sees a report filed by a deployer's reporter (minted and
// filed with the SDK's own clients, as a site's backend and page do), opens
// /queues/<qid>/<tid>, replies and resolves there; a second report filed
// meanwhile appears on the queue's page with no reload, said in its status line.
import { ReporterClient } from "../sdk/src/reporter";
import { ServiceClient } from "../sdk/src/service";
import { expect, startDaemon, test } from "./fixtures";

const SITE = "https://app.example.com";

test("a queue and each of its reports are pages of their own, live", async ({ browser }) => {
  test.setTimeout(60_000);
  const d = await startDaemon();
  try {
    const owner = await browser.newPage();
    await owner.goto(`${d.base}/queues`);
    const made = await owner.evaluate(async site => {
      const post = (p: string, b: unknown) => fetch(p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }).then(r => r.json());
      const q = await post("/api/queues", { name: "Changeling feedback", origins: [site], reporters_see: "own" });
      const g = await post(`/api/queues/${q.queue.id}/service-grants`, { name: "backend" });
      return { qid: q.queue.id as string, token: g.token as string, key: g.private_key as string };
    }, SITE);
    const service = new ServiceClient({ instance: d.base, queue: made.qid, token: made.token, privateKey: made.key });
    // The reporter's page is on the site: its requests carry the site's origin.
    const reporter = new ReporterClient({
      instance: d.base, queue: made.qid,
      auth: publicKey => service.mintReporterToken({ reporter_id: "u1", origin: SITE, public_key: publicKey }),
      fetch: (u, init) => fetch(u, { ...init, headers: { ...(init?.headers as Record<string, string>), origin: SITE } }),
    });
    const file = (body: string) => reporter.fileReport({ url: `${SITE}/invoices/1`, title: "Invoice 1", anchor: { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" }, body });
    const first = await file("The total is wrong");

    // The index links each queue to its own page.
    await owner.reload();
    await owner.getByRole("link", { name: "Changeling feedback" }).click();
    await owner.waitForURL(`${d.base}/queues/${made.qid}`);
    await expect(owner).toHaveTitle("Changeling feedback · Clax");
    await expect(owner.getByRole("heading", { name: "Changeling feedback" })).toBeVisible();
    const card = owner.getByRole("link", { name: /^The total is wrong/ });
    await expect(card).toHaveAttribute("href", `/queues/${made.qid}/${first.id}`);

    // Live: a report filed now appears in place, and is said.
    await file("The date is wrong too");
    await expect(owner.getByRole("link", { name: /^The date is wrong too/ })).toBeVisible();
    await expect(owner.locator(".qv .heard")).toHaveText("1 new report.");

    // The report's own page: its thread, reply and resolve.
    await card.click();
    await owner.waitForURL(`${d.base}/queues/${made.qid}/${first.id}`);
    await expect(owner.getByRole("heading", { name: "The total is wrong" })).toBeVisible();
    await expect(owner).toHaveTitle("Report: The total is wrong · Clax");
    await owner.getByRole("textbox", { name: "Reply" }).fill("Fixed in the next build");
    await owner.getByRole("button", { name: "Send reply" }).click();
    await expect(owner.locator(".rv .said")).toHaveText("Reply sent.");
    await expect(owner.getByText("Fixed in the next build")).toBeVisible();
    await owner.getByRole("button", { name: "Resolve" }).click();
    await expect(owner.locator(".rv .said")).toHaveText("Resolved.");
    await expect(owner.locator(".rv .st")).toHaveText("Resolved");
    // The reporter reads the reply.
    const mine = await reporter.myReports();
    expect(mine.reports.find(r => r.id === first.id)?.comments.map(c => c.body)).toContain("Fixed in the next build");
    await owner.getByRole("link", { name: "← Changeling feedback" }).click();
    await owner.waitForURL(`${d.base}/queues/${made.qid}`);
  } finally {
    await d.stop();
  }
});
