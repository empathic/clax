// The feedback loop end to end (spec 2026-10-29-feedback-loop-design §9,
// §11), on a hosted daemon with a deployer's site on another origin, as
// docs/feedback.md tells an integrator to build it. The site passes the
// overlay a fixed release, flags and an `app.repo`, and ticks "Include
// recent errors" by default (e2e/deployer-site/app.js). The owner's agent
// works through the MCP tools over the daemon's MCP endpoint with an agent
// token, as a remote agent of a hosted Clax does:
//
// 1. the agent watches the queue (`queue_watch`) before anything is filed;
// 2. a site user hits an error, presses S, boxes the problem, reads the
//    release and the error in "What's sent", and sends;
// 3. the agent is woken (`wait_for_feedback`), lists (`queue_list`) and
//    reads (`queue_get`) the report, whose context, flags and diagnostics
//    are what was filed, claims it and proposes a fix (`queue_claim`,
//    `queue_fix`);
// 4. the reporter's badge shows the fix; "Still broken" with a note and an
//    Area capture reopens it, which wakes the agent's wait;
// 5. a second fix; "It's fixed" verifies the report;
// 6. the owner's triage view shows the context card's commit and both fixes.
//
// Screenshots of the mode bar, the annotator, "What's sent", the reporter's
// report view and the triage report view go where queues.spec.ts's go
// ($CLAX_QUEUES_SHOTS_DIR, else the test's output directory).
import type { Page } from "@playwright/test";
import { McpAgent, NATIVE_MCP_PATH } from "../../demo/lib/http";
import type { Report } from "../sdk/src/types";
import { startDeployerSite } from "./deployer-site/server";
import { expect, test } from "./fixtures";
import { hostedDaemon, ownerOf } from "./hosted-helpers";
import { agentTokenOf, cspViolations, openReports, queueWithGrant, refocus, reporterOn, shotPath } from "./queues-helpers";

/** What the site's app.js passes as each capture's context. */
const RELEASE = { version: "2.3.0", commit: "e2e0000", channel: "staging" };
const FLAGS = { single_day_requests: true, approvals: "v2" };
const ERROR_TEXT = "approve req-2: the request has no end date";

/** `queue_get`'s result. */
interface QueueGet {
  report: Report & { clip_url?: string; diagnostics_url?: string | null };
  diagnostics: { capture: number | null; lines: { level: string; kind: string; text: string }[] };
  clip: { capture: number } | null;
}

/** Drags from `a` to `b` in `page`, in steps, as a person draws. */
async function drag(page: Page, a: { x: number; y: number }, b: { x: number; y: number }) {
  await page.mouse.move(a.x, a.y);
  await page.mouse.down();
  await page.mouse.move(b.x, b.y, { steps: 6 });
  await page.mouse.up();
}

test("a report with context and recent errors goes from capture to an agent's fix, is still broken, and is verified on the second fix", async ({ browser }, info) => {
  test.setTimeout(120_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  let agent: McpAgent | null = null;
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    agent = await new McpAgent(origin, NATIVE_MCP_PATH, await agentTokenOf(owner), "claude-code").connect();
    const watched = (await agent.ok("queue_watch", { queue: qid })).data;
    expect(watched).toMatchObject({ watching: true });

    // The reporter hits the error, then reports the visible page.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator("#req-2 .approve").click();
    await emp1.locator(".report-btn").click();
    const bar = emp1.getByRole("toolbar", { name: "Capture" });
    await expect(bar).toBeVisible();
    await emp1.screenshot({ path: shotPath(info, "loop-mode-bar") });
    await emp1.keyboard.press("s");
    const composer = emp1.getByRole("form", { name: "New report" });
    await expect(composer.getByRole("img", { name: "Screenshot of the page" })).toBeVisible();

    // One box around the second request's heading, drawn on the screenshot.
    await composer.getByRole("button", { name: "Box", exact: true }).click();
    const stage = composer.locator(".annotator svg");
    const sb = (await stage.boundingBox())!;
    await drag(emp1, { x: sb.x + sb.width * 0.2, y: sb.y + sb.height * 0.3 }, { x: sb.x + sb.width * 0.6, y: sb.y + sb.height * 0.45 });
    await expect(composer.locator(".annotator svg [data-kind=box]")).toHaveCount(1);
    await emp1.screenshot({ path: shotPath(info, "loop-annotator") });

    // Recent errors start ticked; "What's sent" shows the release, the flags and the error.
    const errors = composer.getByRole("checkbox", { name: "Include recent errors (1)" });
    await expect(errors).toBeChecked();
    await composer.getByText("What's sent").click();
    const sentContext = composer.locator("pre.context");
    await expect(sentContext).toContainText('"commit": "e2e0000"');
    await expect(sentContext).toContainText('"single_day_requests": true');
    await expect(composer.locator("ol.diag")).toContainText(ERROR_TEXT);
    await composer.getByRole("textbox", { name: "Report" }).fill("Approving Tomás's one-day request fails.");
    await composer.locator("details.sent").scrollIntoViewIfNeeded();
    await emp1.screenshot({ path: shotPath(info, "loop-whats-sent") });
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(emp1.getByRole("status", { name: "Filed" })).toBeVisible();

    // The filing wakes the agent's wait, naming the report.
    const woke = await agent.ok("wait_for_feedback", { timeout_s: 60 });
    expect(woke.text).toContain(`This is a new report in queue ${qid}`);

    // The agent lists the queue's active reports: the release and the diagnostics count.
    const listed = (await agent.ok("queue_list", { queue: qid, state: "active" })).data as { reports: (Report & { context_summary?: { release?: unknown; mode?: string } })[] };
    expect(listed.reports).toHaveLength(1);
    const item = listed.reports[0];
    expect(item.context_summary).toMatchObject({ release: RELEASE, mode: "screen" });
    expect(item.diagnostics_lines).toBeGreaterThanOrEqual(1);
    const id = item.id;
    expect(woke.text).toContain(id);

    // It reads the report whole: what the reporter's browser said is what an agent reads.
    const got = (await agent.ok("queue_get", { queue: qid, report_id: id })).data as unknown as QueueGet;
    expect(got.report.context).toMatchObject({ v: 1, release: RELEASE, flags: FLAGS, app: { repo: "example/leave" }, capture: { mode: "screen", method: "dom" }, environment: { url: `${site.origin}/` } });
    expect(got.report.context?.flags).toEqual(FLAGS);
    expect(got.diagnostics.capture).toBe(0);
    expect(got.diagnostics.lines).toContainEqual(expect.objectContaining({ level: "error", text: expect.stringContaining(ERROR_TEXT) }));
    expect(got.clip).toMatchObject({ capture: 0 });

    // The first fix.
    const propose = async (summary: string) => {
      const lease = (await agent!.ok("queue_claim", { queue: qid, report_id: id })).data as { report: Report };
      expect(lease.report.state).toBe("claimed");
      const fixed = (await agent!.ok("queue_fix", { queue: qid, report_id: id, summary, url: "https://github.com/example/leave/pull/31", verify_url: `${site.origin}/` })).data as { report: Report };
      expect(fixed.report.state).toBe("fix_proposed");
    };
    await propose("Single-day requests get their end date from the start date.");

    // The reporter's badge shows it; "Your reports" opens on the waiting fix.
    const count = emp1.locator(".reports-btn .count");
    await refocus(emp1);
    await expect(count).toHaveText("1");
    await emp1.locator(".reports-btn").click();
    const panel = emp1.getByRole("region", { name: "Your reports" });
    const waiting = panel.getByRole("region", { name: "Fix waiting for your check" });
    await expect(waiting).toContainText("Single-day requests get their end date");
    await expect(panel.locator(".state").first()).toHaveText("Fix ready for you to check");
    await emp1.screenshot({ path: shotPath(info, "loop-report-view") });

    // Still broken: a note and an Area capture of the second request. The
    // agent's wait, started first, is woken by the reopen.
    await waiting.getByRole("button", { name: "Still broken" }).click();
    const reopen = emp1.getByRole("form", { name: "Still broken" });
    await expect(reopen).toBeVisible();
    await bar.getByRole("button", { name: "Area", exact: true }).click();
    const req2 = (await emp1.locator("#req-2").boundingBox())!;
    await drag(emp1, { x: req2.x - 4, y: req2.y - 4 }, { x: req2.x + req2.width + 4, y: req2.y + req2.height + 4 });
    await expect(reopen.getByRole("img", { name: /^Screenshot of/ })).toBeVisible();
    await reopen.getByRole("textbox", { name: "What is still broken" }).fill("It still fails on this one.");
    const reopenWait = agent.ok("wait_for_feedback", { timeout_s: 60 });
    await reopen.getByRole("button", { name: "Send" }).click();
    await expect(reopen).toHaveCount(0);
    const reopened = await reopenWait;
    expect(reopened.text).toContain(id);
    expect(JSON.stringify(reopened.data)).toContain("reopened");

    const after = (await agent.ok("queue_get", { queue: qid, report_id: id })).data as unknown as QueueGet;
    expect(after.report).toMatchObject({ state: "open", reopen_count: 1 });
    expect(after.report.fixes?.map(f => f.outcome)).toEqual(["rejected"]);
    expect(after.report.comments.at(-1)).toMatchObject({ author_kind: "reporter", body: "It still fails on this one.", capture: 1, has_clip: true });
    expect(after.clip).toMatchObject({ capture: 1 });

    // The second fix: "It's fixed" verifies it.
    await propose("Approving reads the end date the request was saved with.");
    await refocus(emp1);
    await expect(count).toHaveText("1");
    const mine = await openReports(emp1);
    await mine.getByRole("region", { name: "Fix waiting for your check" }).getByRole("button", { name: "It's fixed" }).click();
    await expect(mine.locator(".state").first()).toHaveText("Fixed: you confirmed it");
    const verified = (await agent.ok("queue_get", { queue: qid, report_id: id })).data as unknown as QueueGet;
    expect(verified.report).toMatchObject({ state: "verified" });
    expect(verified.report.fixes).toHaveLength(2);
    expect(verified.report.fixes?.map(f => f.outcome)).toEqual(["rejected", "verified"]);
    expect(verified.report.fixes?.[1].verdict_context).toMatchObject({ v: 1, release: RELEASE, flags: FLAGS });

    // The owner's triage view: the context card's commit and both fixes.
    await owner.goto(`${origin}/queues/${qid}/${id}`);
    const ctx = owner.getByRole("region", { name: "Context" });
    await expect(ctx.locator(".commit")).toHaveText("e2e0000");
    await expect(ctx.locator(".commit")).toHaveAttribute("href", "https://github.com/example/leave/commit/e2e0000");
    await expect(ctx.locator(".rel")).toContainText("2.3.0");
    await expect(ctx.locator(".flags li")).toHaveText(["approvals: v2", "single_day_requests"]);
    const fixes = owner.getByRole("region", { name: "Fixes" });
    await expect(fixes.locator("li.fix")).toHaveCount(2);
    await expect(fixes.locator("li.fix[data-n='1'] .outcome")).toHaveClass(/rejected/);
    await expect(fixes.locator("li.fix[data-n='2'] .outcome")).toHaveClass(/verified/);
    await owner.screenshot({ path: shotPath(info, "loop-triage-report-view"), fullPage: true });

    // The site's strict policy is never tripped (the overlay is given its nonce).
    expect(await cspViolations(emp1)).toEqual([]);
  } finally {
    await agent?.close();
    await site.stop();
    await d.stop();
  }
});
