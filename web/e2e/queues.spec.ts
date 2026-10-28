// Report queues end to end (plan 2026-10-27-report-queues-m1, Task 8; spec
// 2026-10-27-clax-sdk-design §5.3, §7): on a hosted daemon, the owner makes
// a queue and a service grant on the Queues page; a deployer's site on
// another origin, under a strict Content-Security-Policy, loads the overlay
// from the instance and mints reporter tokens with the grant from its own
// backend; a signed-in user of the site files a report with a clip, with no
// sign-in to Clax; the owner's agent (an agent token, through the triage
// routes) lists, claims, replies to and resolves it; the reporter reads the
// reply in the overlay, and another reporter of the queue does not see it.
//
// Screenshots of the composer (light and dark), "Your reports" and the Queues page go to
// $CLAX_QUEUES_SHOTS_DIR when it is set (under Bazel:
// `--test_env=CLAX_QUEUES_SHOTS_DIR=$PWD/target/e2e-queues`), else to the
// test's output directory.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Browser, Page, TestInfo } from "@playwright/test";
import type { Lease, Report, ReportPage } from "../sdk/src/types";
import { startDeployerSite, type DeployerSite } from "./deployer-site/server";
import { expect, test } from "./fixtures";
import { hostedDaemon, ownerOf } from "./hosted-helpers";

/** Where a screenshot named `name` goes. */
function shotPath(info: TestInfo, name: string): string {
  const dir = process.env.CLAX_QUEUES_SHOTS_DIR;
  if (!dir) return info.outputPath(`${name}.png`);
  mkdirSync(dir, { recursive: true });
  return join(dir, `${name}.png`);
}

/** The owner's agent: an agent token (`Bearer cxa_…`) on the triage routes,
 * from no browser (no `Origin`, no cookie), in a session it registers
 * (`POST /api/sessions`) and names on every call (`X-Clax-Session` and the
 * session's secret): a claim's lease is its session's. `ok` answers a
 * call's JSON and fails the test on any status from 300 up; `bytes`
 * answers a GET's status, type and body. */
async function agentOf(origin: string, token: string) {
  const reg = await fetch(`${origin}/api/sessions`, {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ harness: "claude", harness_session_id: `e2e-queues-${Math.random().toString(36).slice(2)}`, cwd: "/tmp" }),
  });
  const regText = await reg.text();
  expect(reg.status, `POST /api/sessions: ${regText}`).toBe(201);
  const { session, session_secret: secret } = JSON.parse(regText) as { session: { id: string }; session_secret?: string };
  const auth: Record<string, string> = { authorization: `Bearer ${token}`, "x-clax-session": session.id };
  if (secret) auth["x-clax-session-secret"] = secret;
  return {
    sessionId: session.id,
    async ok<T>(method: string, path: string, body?: unknown): Promise<T> {
      const headers = { ...auth, ...(body === undefined ? {} : { "content-type": "application/json" }) };
      const r = await fetch(`${origin}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
      const text = await r.text();
      expect(r.status, `${method} ${path}: ${text}`).toBeLessThan(300);
      return (text ? JSON.parse(text) : null) as T;
    },
    async bytes(path: string): Promise<{ status: number; type: string | null; bytes: Uint8Array }> {
      const r = await fetch(`${origin}${path}`, { headers: auth });
      return { status: r.status, type: r.headers.get("content-type"), bytes: new Uint8Array(await r.arrayBuffer()) };
    },
  };
}

/** A user `user` of the deployer's site, signed in to the site (not to
 * Clax) in a browser of their own, on its page with the overlay loaded.
 * Every CSP violation the page reports is kept in `window.cspViolations`
 * as its directive, blocked URI and sample (JSON). */
async function reporterOn(browser: Browser, site: DeployerSite, user: string): Promise<Page> {
  const page = await (await browser.newContext()).newPage();
  await page.addInitScript(() => {
    (window as unknown as { cspViolations: string[] }).cspViolations = [];
    document.addEventListener("securitypolicyviolation", e => {
      (window as unknown as { cspViolations: string[] }).cspViolations.push(`${e.violatedDirective} ${e.blockedURI} ${JSON.stringify(e.sample)}`);
    });
  });
  await page.goto(`${site.origin}/login?user=${user}`);
  await page.waitForURL(`${site.origin}/`);
  await expect(page.locator(".report-btn")).toBeVisible();
  return page;
}

const cspViolations = (p: Page) => p.evaluate(() => (window as unknown as { cspViolations: string[] }).cspViolations);

/** "Your reports" in `page`'s overlay, opened afresh (opening loads the
 * reporter's reports at once; the panel's own poll is 15 s). */
async function openReports(page: Page) {
  const btn = page.locator(".reports-btn");
  if ((await btn.getAttribute("aria-expanded")) === "true") await btn.click();
  await expect(btn).toHaveAttribute("aria-expanded", "false");
  await btn.click();
  const panel = page.getByRole("region", { name: "Your reports" });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("Loading…")).toHaveCount(0);
  return panel;
}

const PNG_MAGIC = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

test("a site's user files a report through the overlay; an agent claims, replies and resolves; only that reporter reads it", async ({ browser }, info) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    // The owner makes the queue, for the site's origin, reporters seeing
    // their own reports.
    const owner = await ownerOf(browser, origin);
    await owner.goto(`${origin}/queues`);
    await owner.getByRole("button", { name: "New queue" }).click();
    const form = owner.locator("form.qform");
    await form.getByLabel("Name", { exact: true }).fill("Leave app reports");
    await form.getByLabel("Origins, one per line").fill(site.origin);
    await form.getByLabel("Their own reports").check();
    await form.getByRole("button", { name: "Create queue" }).click();
    const card = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Leave app reports" }) });
    await expect(card).toBeVisible();
    await expect(card.getByRole("list", { name: "Origins of Leave app reports" })).toHaveText(site.origin);
    const qid = (await card.getAttribute("data-id"))!;
    expect(qid).toBeTruthy();

    // A service grant: its token and private key, shown once.
    await card.getByRole("button", { name: "Manage" }).click();
    await card.getByLabel("Name", { exact: true }).fill("leave app backend");
    await card.getByRole("button", { name: "Create service grant" }).click();
    const secret = card.getByRole("region", { name: "New service grant" });
    await expect(secret).toBeVisible();
    const token = (await secret.locator("code").nth(0).textContent())!.trim();
    const privateKey = (await secret.locator("code").nth(1).textContent())!.trim();
    expect(token).toMatch(/^cxq_/);
    expect(privateKey).toMatch(/^[A-Za-z0-9_-]+$/);
    await owner.screenshot({ path: shotPath(info, "queues-grant"), fullPage: true });
    await secret.getByRole("button", { name: "Done" }).click();
    await expect(secret).toHaveCount(0);
    await expect(card.locator("li.line[data-grant]")).toHaveCount(1);
    site.grant({ queue: qid, token, privateKey });

    // emp-1, signed in to the site only, picks the first request's heading
    // and files a report with its clip.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator(".report-btn").click();
    await expect(emp1.getByText("Click what your report is about")).toBeVisible();
    await emp1.locator("#req-1 h2").click();
    const composer = emp1.getByRole("form", { name: "New report" });
    await expect(composer).toBeVisible();
    // The overlay's own styles apply under the policy (no inline style).
    await expect(emp1.locator(".launcher")).toHaveCSS("display", "flex");
    await expect(composer).toHaveCSS("border-top-width", "3px");
    await expect(composer.getByRole("img", { name: "Screenshot of the picked part of the page" })).toBeVisible();
    const body = "The dates read 3 to 7 November but the request was for 4 days: which is right?";
    await composer.getByRole("textbox", { name: "Report" }).fill(body);
    await emp1.screenshot({ path: shotPath(info, "overlay-composer") });
    // The buttons' colours ease (tokens.css --t): each shot waits for Send's to settle.
    const send = composer.getByRole("button", { name: "Send" });
    await emp1.emulateMedia({ colorScheme: "dark" });
    await expect(send).toHaveCSS("background-color", "rgb(236, 235, 231)");
    await emp1.screenshot({ path: shotPath(info, "overlay-composer-dark") });
    await emp1.emulateMedia({ colorScheme: "light" });
    await expect(send).toHaveCSS("background-color", "rgb(28, 27, 25)");
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(emp1.getByText("Report sent.")).toBeVisible();
    await expect(composer).toHaveCount(0);
    await expect(emp1.locator(".report-btn")).toBeFocused();
    expect(site.minted).toContain("emp-1");

    // The owner's agent, with an agent token made in the owner's browser.
    const agentToken = await owner.evaluate(async () => {
      const r = await fetch("/api/hosted/agent-tokens", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ label: "triage-laptop" }) });
      if (!r.ok) throw new Error(`agent token: ${r.status} ${await r.text()}`);
      return (await r.json()).token as string;
    });
    const agent = await agentOf(origin, agentToken);
    const listed = await agent.ok<ReportPage>("GET", `/api/queues/${qid}/reports`);
    expect(listed.reports).toHaveLength(1);
    const report = listed.reports[0];
    expect(report).toMatchObject({ queue_id: qid, reporter_id: "emp-1", has_clip: true, status: "open", state: "open", claim: null });
    expect(report.comments).toHaveLength(1);
    expect(report.comments[0]).toMatchObject({ author_kind: "reporter", body });
    // No external ID: the report's artifact is the page, by its URL.
    expect(report.artifact.url).toBe(`${site.origin}/`);
    expect(report.artifact.external_id).toBeTruthy();

    // Its clip is a PNG with a size.
    const clip = await agent.bytes(`/api/queues/${qid}/reports/${report.id}/clip`);
    expect(clip.status).toBe(200);
    expect(clip.type).toContain("image/png");
    expect([...clip.bytes.slice(0, 8)]).toEqual(PNG_MAGIC);
    const view = new DataView(clip.bytes.buffer, clip.bytes.byteOffset);
    expect(view.getUint32(16)).toBeGreaterThan(0);
    expect(view.getUint32(20)).toBeGreaterThan(0);

    // The agent claims it (a lease) and replies under the lease.
    const lease = await agent.ok<Lease>("POST", `/api/queues/${qid}/reports/${report.id}/claim`);
    expect(lease.lease_id).toBeTruthy();
    expect(typeof lease.fence).toBe("number");
    expect(Date.parse(lease.expires_at)).toBeGreaterThan(Date.now());
    expect(lease.report).toMatchObject({ id: report.id, state: "claimed" });
    expect(lease.report.claim).not.toBeNull();
    const reply = "The request is for 3 to 6 November; the end date is fixed in the next deploy.";
    await agent.ok("POST", `/api/queues/${qid}/reports/${report.id}/comments`, { body: reply });

    // emp-1 reads the reply in "Your reports", and answers it.
    const mine = await openReports(emp1);
    const row = mine.getByRole("listitem").filter({ hasText: "The dates read 3 to 7 November" });
    await expect(row).toHaveCount(1);
    await expect(row.locator(".state")).toHaveText("Being worked on");
    await expect(row).toContainText("1 reply");
    await row.getByRole("button", { name: /The dates read/ }).click();
    await expect(row.locator(".c[data-kind=agent] .body")).toHaveText(reply);
    await emp1.screenshot({ path: shotPath(info, "overlay-your-reports") });
    await row.getByRole("textbox", { name: "Reply" }).fill("Thanks, 3 to 6 is right.");
    await row.getByRole("button", { name: "Reply", exact: true }).click();
    await expect(row.locator(".c[data-kind=reporter] .body").last()).toHaveText("Thanks, 3 to 6 is right.");
    const read = await agent.ok<{ report: Report }>("GET", `/api/queues/${qid}/reports/${report.id}`);
    expect(read.report.comments.map(c => [c.author_kind, c.body])).toEqual([["reporter", body], ["agent", reply], ["reporter", "Thanks, 3 to 6 is right."]]);

    // emp-2, another user of the site, sees no report of emp-1's.
    const emp2 = await reporterOn(browser, site, "emp-2");
    const theirs = await openReports(emp2);
    await expect(theirs.getByText("No reports yet.")).toBeVisible();
    await expect(theirs).not.toContainText("The dates read");
    expect(site.minted).toContain("emp-2");

    // The agent resolves it: the agent and emp-1 both read it resolved.
    await agent.ok("POST", `/api/queues/${qid}/reports/${report.id}/resolve`);
    const done = await agent.ok<{ report: Report }>("GET", `/api/queues/${qid}/reports/${report.id}`);
    expect(done.report).toMatchObject({ status: "resolved", state: "resolved" });
    const again = await openReports(emp1);
    const resolvedRow = again.getByRole("listitem").filter({ hasText: "The dates read 3 to 7 November" });
    await expect(resolvedRow.locator(".state")).toHaveText("Resolved");
    await resolvedRow.getByRole("button", { name: /The dates read/ }).click();
    await expect(resolvedRow.getByRole("button", { name: "Reply", exact: true })).toHaveCount(0);

    // The Queues page counts it resolved.
    await owner.reload();
    const counted = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Leave app reports" }) });
    await expect(counted.locator(".counts")).toHaveText("0 open · 0 claimed · 1 resolved");
    await owner.screenshot({ path: shotPath(info, "queues-page"), fullPage: true });

    // The overlay ran under the site's policy without tripping it, but for
    // the one empty <style> the first clip's renderer (modern-screenshot)
    // reads web fonts through, which the policy refuses; that page's clips
    // are then rendered without web fonts (web/bridge/src/clip.ts).
    expect(await cspViolations(emp1)).toEqual(['style-src-elem inline ""']);
    expect(await cspViolations(emp2)).toEqual([]);
  } finally {
    await site.stop();
    await d.stop();
  }
});
