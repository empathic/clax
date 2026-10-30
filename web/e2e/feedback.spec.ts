// clax.dev reports problems with itself (spec 2026-10-29-feedback-loop
// §10): a hosted daemon configured with no `[feedback]` makes its own queue
// (the owner's, for its public origin) and service grant the first time a
// person's shell asks, and the shell loads the overlay from the instance
// and mints reporter tokens through its own backend route. The owner opens
// the header's feedback menu, starts a report with the shortcut, files the
// visible page, and the report carries their public ID and the shell's
// context; the badge on the menu follows a proposed fix; the queue is on
// the owner's Queues page. A connected person, with no flag of any kind,
// gets the menu too, from the same queue. No Content Security Policy
// violation is reported on the way.
import type { Page } from "@playwright/test";
import type { Lease, Report, ReportPage } from "../sdk/src/types";
import { expect, test } from "./fixtures";
import { hostedDaemon, memberOf, ownerOf } from "./hosted-helpers";

/** Keeps every CSP violation `page` reports in `window.cspViolations`. */
async function watchCsp(page: Page) {
  await page.addInitScript(() => {
    (window as unknown as { cspViolations: string[] }).cspViolations = [];
    document.addEventListener("securitypolicyviolation", e => {
      (window as unknown as { cspViolations: string[] }).cspViolations.push(`${e.violatedDirective} ${e.blockedURI}`);
    });
  });
}
const cspViolations = (p: Page) => p.evaluate(() => (window as unknown as { cspViolations: string[] }).cspViolations);

/** `page`'s `fetch` of `path` (same origin, its session), answering status and JSON. */
const call = (page: Page, method: string, path: string, body?: unknown) => page.evaluate(async ([m, p, b]) => {
  const r = await fetch(p, { method: m, headers: b === undefined ? {} : { "content-type": "application/json" }, body: b === undefined ? undefined : JSON.stringify(b) });
  const text = await r.text();
  return { status: r.status, json: text ? JSON.parse(text) : null };
}, [method, path, body] as const);

test("on a hosted instance with nothing configured everyone files a report about Clax from its shell; the menu's badge follows the fix", async ({ browser }) => {
  test.setTimeout(120_000);
  const { d, origin } = await hostedDaemon();
  try {
    const owner = await ownerOf(browser, origin);
    const agentToken = await call(owner, "POST", "/api/hosted/agent-tokens", { label: "triage" });
    expect(agentToken.status).toBeLessThan(300);
    const auth = { authorization: `Bearer ${agentToken.json.token}` };
    const agent = async <T>(method: string, path: string, body?: unknown): Promise<T> => {
      const r = await fetch(`${origin}${path}`, { method, headers: { ...auth, ...(body === undefined ? {} : { "content-type": "application/json" }) }, body: body === undefined ? undefined : JSON.stringify(body) });
      const text = await r.text();
      expect(r.status, `${method} ${path}: ${text}`).toBeLessThan(300);
      return JSON.parse(text) as T;
    };
    const sessions = await agent<{ session: { id: string }; session_secret?: string }>("POST", "/api/sessions", { harness: "claude", harness_session_id: `e2e-feedback-${Math.random().toString(36).slice(2)}`, cwd: "/tmp" });
    Object.assign(auth, { "x-clax-session": sessions.session.id }, sessions.session_secret ? { "x-clax-session-secret": sessions.session_secret } : {});

    await watchCsp(owner);
    // The instance is the shell's own origin: the overlay's same-origin
    // reads (the badge, the reports) carry no `Origin`, and none is refused.
    const refused: string[] = [];
    owner.on("response", r => { if (r.url().includes("/reporter/") && r.status() >= 400) refused.push(`${r.status()} ${new URL(r.url()).pathname}`); });
    const requests: string[] = [];
    owner.on("request", r => { requests.push(new URL(r.url()).pathname); });
    await owner.goto(`${origin}/`);
    const menuButton = owner.getByRole("button", { name: /^Feedback/ });
    await expect(menuButton).toBeVisible();
    expect(requests).toContain("/_app/feedback/config");
    expect(requests).toContain("/_clax/overlay.js");

    // The instance's own queue, made as the shell asked: the owner's, for the
    // public origin, on their Queues page.
    const config = (await call(owner, "GET", "/_app/feedback/config")).json;
    expect(config).toMatchObject({ enabled: true, instance: origin });
    const qid = config.queue as string;
    expect((await call(owner, "GET", "/api/queues")).json.queues).toMatchObject([{ id: qid, name: "clax-feedback", role: "admin", origins: [origin], reporters_see: "own" }]);

    // The menu names the shortcut; the shortcut starts a report.
    await menuButton.click();
    const report = owner.getByRole("menuitem", { name: "Report a problem" });
    await expect(report).toBeVisible();
    await expect(report).toHaveAttribute("aria-keyshortcuts", "F");
    await expect(report.locator("kbd")).toHaveText("F");
    await owner.keyboard.press("Escape");
    await expect(report).toHaveCount(0);
    // Typed into the search box, F is the field's.
    const search = owner.getByRole("searchbox", { name: "Search artifacts" });
    await search.focus();
    await owner.keyboard.press("f");
    await expect(search).toHaveValue("f");
    const bar = owner.getByRole("toolbar", { name: "Capture" });
    await expect(bar).toHaveCount(0);
    await search.fill("");
    await owner.evaluate(() => { (document.activeElement as HTMLElement | null)?.blur(); });
    await owner.keyboard.press("f");
    await expect(bar).toBeVisible();
    await owner.keyboard.press("s");
    const composer = owner.getByRole("form", { name: "New report" });
    await expect(composer.getByRole("img", { name: "Screenshot of the page" })).toBeVisible();
    await composer.getByRole("textbox", { name: "Report" }).fill("The gallery's tag bar overlaps the first card.");
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(owner.getByRole("status", { name: "Filed" })).toBeVisible();

    // The report: the owner's public ID, the shell's release, flags and view.
    const me = await call(owner, "GET", "/api/viewers/me");
    const listed = await agent<ReportPage>("GET", `/api/queues/${qid}/reports`);
    expect(listed.reports).toHaveLength(1);
    const id = listed.reports[0].id;
    const filed = (await agent<{ report: Report }>("GET", `/api/queues/${qid}/reports/${id}`)).report;
    const health = await (await fetch(`${origin}/healthz`)).json() as { version: string };
    expect(filed.reporter_id).toBe(me.json.viewer.public_id);
    expect(filed.context).toMatchObject({
      v: 1,
      release: { version: health.version, channel: "dev" },
      flags: { claxbot: true, claxbot_unenforced: false },
      app: { repo: "https://github.com/empathic/clax", view: "gallery", artifact: null },
      capture: { mode: "screen" },
      environment: { url: `${origin}/` },
    });

    // A fix proposed: the menu's button and My reports show it.
    const lease = await agent<Lease>("POST", `/api/queues/${qid}/reports/${id}/claim`);
    await agent("POST", `/api/queues/${qid}/reports/${id}/fix`, { summary: "The tag bar wraps above the grid now.", verify_url: `${origin}/`, fence: lease.fence });
    // Focus asks the overlay for the badge at once; a focus heard while an
    // earlier load is in flight is answered by that load, which may predate
    // the fix, so focus is repeated until a badge answer counts it.
    for (;;) {
      const answer = owner.waitForResponse(r => /\/reporter\/badge$/.test(new URL(r.url()).pathname));
      await owner.evaluate(() => { window.dispatchEvent(new Event("focus")); });
      const badge = await (await answer).json() as { waiting: number };
      if (badge.waiting === 1) break;
    }
    await expect(menuButton).toHaveAccessibleName("Feedback: 1 report waits on you");
    await menuButton.click();
    await owner.getByRole("menuitem", { name: "My reports: 1 report waits on you" }).click();
    const panel = owner.getByRole("region", { name: "Your reports" });
    await expect(panel.getByRole("region", { name: "Fix waiting for your check" })).toContainText("The tag bar wraps above the grid now.");

    // The shell's policy restricts no style or script source and the shell
    // sets its type in system faces, so the overlay needs no `nonce`, and
    // mounting and clipping trip nothing.
    expect(await cspViolations(owner)).toEqual([]);
    expect(refused).toEqual([]);

    // A connected person no admin has touched: the menu, on the same queue.
    const ben = await memberOf(browser, owner, origin, "Ben", []);
    await ben.goto(`${origin}/`);
    await expect(ben.getByRole("button", { name: /^Feedback/ })).toBeVisible();
    expect((await call(ben, "GET", "/_app/feedback/config")).json).toMatchObject({ enabled: true, instance: origin, queue: qid });
    expect((await call(owner, "GET", "/api/queues")).json.queues).toHaveLength(1);
  } finally {
    await d.stop();
  }
});
