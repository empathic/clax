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
    const filedNote = emp1.getByRole("status", { name: "Filed" });
    await expect(filedNote).toBeVisible();
    await expect(composer).toHaveCount(0);
    await expect(filedNote.getByRole("button", { name: "View in Your reports" })).toBeFocused();
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
    await expect(counted.locator(".counts")).toHaveText("0 open · 0 claimed · 0 fix proposed · 0 verified · 1 resolved");
    await owner.screenshot({ path: shotPath(info, "queues-page"), fullPage: true });

    // The queue's view lists it under Resolved, and its report view shows the conversation.
    await counted.getByRole("link", { name: "Leave app reports" }).click();
    await expect(owner).toHaveURL(new RegExp(`/queues/${qid}$`));
    await expect(owner.getByText("No active reports.")).toBeVisible();
    await owner.getByRole("tab", { name: "Resolved" }).click();
    const triaged = owner.locator(`li.rrow[data-id="${report.id}"]`);
    await expect(triaged.locator(".state")).toHaveText("Resolved");
    await triaged.getByRole("link").click();
    await expect(owner).toHaveURL(new RegExp(`/queues/${qid}/${report.id}\\?state=resolved$`));
    await expect(owner.locator(".rview .note .body")).toHaveText(body);
    await expect(owner.locator(".convo .cm .body")).toHaveText([reply, "Thanks, 3 to 6 is right."]);
    await expect(owner.getByRole("button", { name: "Reopen for the reporter" })).toBeVisible();
    await owner.screenshot({ path: shotPath(info, "report-view"), fullPage: true });
    await owner.locator(".rview .back a").click();
    await expect(owner).toHaveURL(new RegExp(`/queues/${qid}\\?state=resolved$`));
    await expect(owner.getByRole("tab", { name: "Resolved" })).toHaveAttribute("aria-selected", "true");
    await expect(owner.locator(`li.rrow[data-id="${report.id}"]`)).toBeVisible();

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

/** A queue for `site`'s origin, reporters seeing their own reports, made
 * by `owner` on the Queues page, with a service grant handed to the site. */
async function queueWithGrant(owner: Page, origin: string, site: DeployerSite): Promise<string> {
  await owner.goto(`${origin}/queues`);
  await owner.getByRole("button", { name: "New queue" }).click();
  const form = owner.locator("form.qform");
  await form.getByLabel("Name", { exact: true }).fill("Leave app reports");
  await form.getByLabel("Origins, one per line").fill(site.origin);
  await form.getByLabel("Their own reports").check();
  await form.getByRole("button", { name: "Create queue" }).click();
  const card = owner.locator("li.qcard", { has: owner.getByRole("heading", { name: "Leave app reports" }) });
  await expect(card).toBeVisible();
  const qid = (await card.getAttribute("data-id"))!;
  await card.getByRole("button", { name: "Manage" }).click();
  await card.getByLabel("Name", { exact: true }).fill("leave app backend");
  await card.getByRole("button", { name: "Create service grant" }).click();
  const secret = card.getByRole("region", { name: "New service grant" });
  await expect(secret).toBeVisible();
  const token = (await secret.locator("code").nth(0).textContent())!.trim();
  const privateKey = (await secret.locator("code").nth(1).textContent())!.trim();
  await secret.getByRole("button", { name: "Done" }).click();
  site.grant({ queue: qid, token, privateKey });
  return qid;
}

/** Tells `page`'s overlay the page has focus again, which asks for the badge at once. */
const refocus = (page: Page) => page.evaluate(() => { window.dispatchEvent(new Event("focus")); });

test("a reporter files the visible page, says a fix is still broken with a new capture, then confirms the next fix", async ({ browser }) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    const agentToken = await owner.evaluate(async () => {
      const r = await fetch("/api/hosted/agent-tokens", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ label: "triage-laptop" }) });
      if (!r.ok) throw new Error(`agent token: ${r.status} ${await r.text()}`);
      return (await r.json()).token as string;
    });
    const agent = await agentOf(origin, agentToken);

    // Screen mode, from the mode bar's S key: the visible page at once.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator(".report-btn").click();
    const bar = emp1.getByRole("toolbar", { name: "Capture" });
    await expect(bar.getByRole("button", { name: "Element", exact: true })).toHaveAttribute("aria-pressed", "true");
    await emp1.keyboard.press("s");
    await expect(bar).toHaveCount(0);
    const composer = emp1.getByRole("form", { name: "New report" });
    await expect(composer.getByRole("img", { name: "Screenshot of the page" })).toBeVisible();
    await composer.getByText("What's sent").click();
    await expect(composer.locator("pre.context")).toContainText('"mode": "screen"');
    await expect(composer.getByText("Recent errors are not sent.")).toBeVisible();
    await composer.getByRole("textbox", { name: "Report" }).fill("The second request has no end date.");
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(emp1.getByRole("status", { name: "Filed" })).toBeVisible();

    const listed = await agent.ok<ReportPage>("GET", `/api/queues/${qid}/reports`);
    expect(listed.reports).toHaveLength(1);
    const id = listed.reports[0].id;
    const filed = await agent.ok<{ report: Report }>("GET", `/api/queues/${qid}/reports/${id}`);
    expect(filed.report).toMatchObject({ has_clip: true, anchor: { kind: "artifact" }, context: { v: 1, capture: { mode: "screen", method: "dom" }, environment: { url: `${site.origin}/` } } });

    // A fix is proposed: the badge rises, and "Your reports" opens on it.
    const propose = async (summary: string) => {
      const lease = await agent.ok<Lease>("POST", `/api/queues/${qid}/reports/${id}/claim`);
      await agent.ok("POST", `/api/queues/${qid}/reports/${id}/fix`, { summary, url: "https://github.com/example/leave/pull/12", verify_url: `${site.origin}/`, fence: lease.fence });
    };
    const count = emp1.locator(".reports-btn .count");
    await refocus(emp1);
    await expect(count).toHaveCount(0);
    await propose("The end date shows on every request now.");
    await refocus(emp1);
    await expect(count).toHaveText("1");
    await emp1.locator(".reports-btn").click();
    const panel = emp1.getByRole("region", { name: "Your reports" });
    const waiting = panel.getByRole("region", { name: "Fix waiting for your check" });
    await expect(waiting).toContainText("The end date shows on every request now.");
    await expect(waiting.getByRole("link", { name: "The change" })).toHaveAttribute("href", "https://github.com/example/leave/pull/12");
    await expect(panel.locator(".state").first()).toHaveText("Fix ready for you to check");

    // Still broken: a note and a new capture of the second request.
    await waiting.getByRole("button", { name: "Still broken" }).click();
    const reopen = emp1.getByRole("form", { name: "Still broken" });
    await expect(reopen).toBeVisible();
    await bar.getByRole("button", { name: "Element", exact: true }).click();
    await emp1.locator("#req-2 h2").click();
    await expect(reopen.getByRole("img", { name: "Screenshot of the page" })).toBeVisible();
    await reopen.getByRole("textbox", { name: "What is still broken" }).fill("Still no end date on this one.");
    await reopen.getByRole("button", { name: "Send" }).click();
    await expect(reopen).toHaveCount(0);
    const reopened = await agent.ok<{ report: Report }>("GET", `/api/queues/${qid}/reports/${id}`);
    expect(reopened.report).toMatchObject({ state: "open", reopen_count: 1 });
    expect(reopened.report.fixes?.map(f => f.outcome)).toEqual(["rejected"]);
    expect(reopened.report.comments.at(-1)).toMatchObject({ author_kind: "reporter", body: "Still no end date on this one.", capture: 1, has_clip: true });

    // The next fix: "It's fixed" verifies it, with the reporter's context.
    await refocus(emp1);
    await expect(count).toHaveCount(0);
    await propose("The end date shows on single-day requests too.");
    await refocus(emp1);
    await expect(count).toHaveText("1");
    const mine = await openReports(emp1);
    await mine.getByRole("region", { name: "Fix waiting for your check" }).getByRole("button", { name: "It's fixed" }).click();
    await expect(mine.locator(".state").first()).toHaveText("Fixed: you confirmed it");
    const verified = await agent.ok<{ report: Report }>("GET", `/api/queues/${qid}/reports/${id}`);
    expect(verified.report).toMatchObject({ state: "verified", status: "resolved" });
    expect(verified.report.fixes?.map(f => f.outcome)).toEqual(["rejected", "verified"]);
    expect(verified.report.fixes?.[1].verdict_context).toMatchObject({ v: 1, environment: { url: `${site.origin}/` } });

    // Only the clip renderer's empty font <style> trips the page's policy.
    expect(await cspViolations(emp1)).toEqual(['style-src-elem inline ""']);
  } finally {
    await site.stop();
    await d.stop();
  }
});

test("a redaction drawn on a tall capture, scrolled in its stage, lands where it was drawn in the PNG sent", async ({ browser }) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    const agentToken = await owner.evaluate(async () => {
      const r = await fetch("/api/hosted/agent-tokens", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ label: "triage-laptop" }) });
      if (!r.ok) throw new Error(`agent token: ${r.status} ${await r.text()}`);
      return (await r.json()).token as string;
    });
    const agent = await agentOf(origin, agentToken);

    // Pick the tall section from the keyboard: Down past each element (Right
    // into the page's main), Enter on the one the mode bar announces.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator(".report-btn").click();
    const announced = emp1.locator(".modebar [aria-live]");
    let last = "";
    for (let i = 0; i < 20 && !last.startsWith("Outlined section: Earlier requests"); i++) {
      await emp1.keyboard.press("ArrowDown");
      let now = (await announced.textContent()) ?? "";
      if (now === last) {
        await emp1.keyboard.press("ArrowRight");
        now = (await announced.textContent()) ?? "";
      }
      last = now;
    }
    expect(last).toMatch(/^Outlined section: Earlier requests/);
    await emp1.keyboard.press("Enter");
    const composer = emp1.getByRole("form", { name: "New report" });
    const img = composer.getByRole("img", { name: "Screenshot of the picked part of the page" });
    await expect(img).toBeVisible();
    await expect.poll(() => img.evaluate(i => (i as HTMLImageElement).naturalHeight)).toBeGreaterThan(1000);

    // Scroll the stage well down, and drag a redaction around its middle.
    await composer.getByRole("button", { name: "Clear", exact: true }).click();
    await composer.getByRole("button", { name: "Redact", exact: true }).click();
    const stage = composer.locator(".annotator .stage");
    await stage.evaluate(el => { el.scrollTop = 300; });
    await expect.poll(() => stage.evaluate(el => el.scrollTop)).toBe(300);
    const sb = (await stage.boundingBox())!;
    const mid = { x: sb.x + sb.width / 2, y: sb.y + sb.height / 2 };
    await emp1.mouse.move(mid.x - 30, mid.y - 20);
    await emp1.mouse.down();
    await emp1.mouse.move(mid.x + 30, mid.y + 20, { steps: 5 });
    await emp1.mouse.up();
    await expect(composer.locator(".annotator svg [data-kind=redact]")).toHaveCount(1);
    // Where the middle of the drag is in the image's own pixels, read off the drawing.
    const geo = await composer.locator(".annotator svg").evaluate(el => {
      const r = el.getBoundingClientRect();
      const i = el.previousElementSibling as HTMLImageElement;
      return { x: r.x, y: r.y, w: r.width, h: r.height, nw: i.naturalWidth, nh: i.naturalHeight };
    });
    expect(geo.h).toBeGreaterThan(sb.height + 300);
    const at = { x: Math.round(((mid.x - geo.x) / geo.w) * geo.nw), y: Math.round(((mid.y - geo.y) / geo.h) * geo.nh) };
    // Where it would land were the drawing the stage's visible box, not the image's.
    const stuck = { x: at.x, y: Math.round(((mid.y - sb.y) / geo.h) * geo.nh) };
    expect(at.y - stuck.y).toBeGreaterThan(100);

    await composer.getByRole("textbox", { name: "Report" }).fill("The history shows a name it should not.");
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(emp1.getByRole("status", { name: "Filed" })).toBeVisible();
    const listed = await agent.ok<ReportPage>("GET", `/api/queues/${qid}/reports`);
    const clip = await agent.bytes(`/api/queues/${qid}/reports/${listed.reports[0].id}/clip`);
    expect(clip.status).toBe(200);
    const pixels = await emp1.evaluate(async ({ b64, points }) => {
      const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
      const bmp = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const c = new OffscreenCanvas(bmp.width, bmp.height);
      const g = c.getContext("2d")!;
      g.drawImage(bmp, 0, 0);
      return { size: [bmp.width, bmp.height], at: points.map(p => [...g.getImageData(p.x, p.y, 1, 1).data]) };
    }, { b64: Buffer.from(clip.bytes).toString("base64"), points: [at, stuck] });
    expect(pixels.size).toEqual([geo.nw, geo.nh]);
    expect(pixels.at[0]).toEqual([0, 0, 0, 255]);
    expect(pixels.at[1]).not.toEqual([0, 0, 0, 255]);
  } finally {
    await site.stop();
    await d.stop();
  }
});
