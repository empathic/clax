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
import { join } from "node:path";
import type { Browser, Page } from "@playwright/test";
import type { Lease, Report, ReportPage } from "../sdk/src/types";
import { type DeployerSite, startDeployerSite } from "./deployer-site/server";
import { fakeMessages, finishTurn } from "./fake-messages";
import { NO_KEY_CONFIG, expect, startDaemon, test } from "./fixtures";
import { hostedDaemon, ownerOf } from "./hosted-helpers";
import { agentOf, agentTokenOf, cspViolations, openReports, queueWithGrant, refocus, reporterOn, shotPath } from "./queues-helpers";

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

    // Its repository: connected with tokens, which come back as metadata
    // alone, each beside the hosts it goes to.
    const repo = card.getByRole("region", { name: "Repository of Leave app reports" });
    await expect(repo.getByText("Not connected.")).toBeVisible();
    await repo.getByLabel("Repository", { exact: true }).fill("acme/leave");
    await repo.getByLabel("Preview URL").fill("https://pr-{pr}.acme.dev");
    await repo.getByRole("button", { name: "Connect" }).click();
    await expect(repo.getByText("Not ready: add a write token and a read token below.")).toBeVisible();
    const write = repo.locator('[data-token="write"]');
    await expect(write.locator("[data-hosts]")).toHaveText("Sent to github.com and api.github.com.");
    await write.getByRole("button", { name: /^Set/ }).click();
    await write.locator('input[type="password"]').fill("github_pat_e2e_0123456789_wxyz");
    await write.getByRole("button", { name: "Save" }).click();
    await expect(write.getByText("Set, ending wxyz")).toBeVisible();
    // A private repository needs a read token too, and the panel says so.
    await expect(repo.getByText("Not ready: add a read token below.")).toBeVisible();
    await expect(repo).not.toContainText("github_pat_e2e");
    await owner.screenshot({ path: shotPath(info, "queues-repository"), fullPage: true });
    owner.once("dialog", dialog => void dialog.accept());
    await repo.getByRole("button", { name: "Disconnect" }).click();
    await expect(repo.getByText("Not connected.")).toBeVisible();

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
    const agentToken = await agentTokenOf(owner);
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

    // The overlay ran under the site's strict policy without tripping it:
    // told the page's nonce, its clips never read web fonts through an
    // inline <style> (web/bridge/src/clip.ts withoutWebFonts).
    expect(await cspViolations(emp1)).toEqual([]);
    expect(await cspViolations(emp2)).toEqual([]);
  } finally {
    await site.stop();
    await d.stop();
  }
});

test("a reporter files the visible page, says a fix is still broken with a new capture, then confirms the next fix", async ({ browser }) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    const agentToken = await agentTokenOf(owner);
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

    // The strict policy is never tripped.
    expect(await cspViolations(emp1)).toEqual([]);
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
    const agentToken = await agentTokenOf(owner);
    const agent = await agentOf(origin, agentToken);

    // Pick the tall section from the keyboard: E again (Element is on)
    // starts it, then Down past each element (Right into the page's main),
    // Enter on the one the mode bar announces.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator(".report-btn").click();
    await emp1.keyboard.press("e");
    const announced = emp1.locator(".modebar [aria-live]");
    let last = (await announced.textContent()) ?? "";
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
    expect(await cspViolations(emp1)).toEqual([]);
  } finally {
    await site.stop();
    await d.stop();
  }
});

test("the triage views stay current: a filing, an agent's claim and a person's release show without a reload", async ({ browser }) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    const agent = await agentOf(origin, await agentTokenOf(owner));
    await owner.goto(`${origin}/queues/${qid}`);
    await expect(owner.locator(".qnone")).toHaveText("No active reports.");

    // A site user files a report: the open queue view lists it.
    const emp1 = await reporterOn(browser, site, "emp-1");
    await emp1.locator(".report-btn").click();
    await expect(emp1.getByText("Click what your report is about")).toBeVisible();
    await emp1.locator("#req-1 h2").click();
    const composer = emp1.getByRole("form", { name: "New report" });
    await composer.getByRole("textbox", { name: "Report" }).fill("The dates are off by one.");
    await composer.getByRole("button", { name: "Send" }).click();
    await expect(emp1.getByRole("status", { name: "Filed" })).toBeVisible();
    const row = owner.locator("li.rrow");
    await expect(row).toHaveCount(1);
    await expect(row.locator(".note")).toHaveText("The dates are off by one.");
    const id = (await row.getAttribute("data-id"))!;

    // The report view: the agent's claim shows, and a half-written reply stays.
    await row.locator("a").click();
    await expect(owner.locator(".rh .state")).toHaveText("Open");
    const reply = owner.locator("#rv-reply");
    await reply.fill("Half a reply");
    const lease = await agent.ok<Lease>("POST", `/api/queues/${qid}/reports/${id}/claim`);
    await expect(owner.locator(".rh .state")).toHaveText("Claimed");
    await expect(owner.locator(".by .claim")).toContainText("claimed by");
    await expect(reply).toHaveValue("Half a reply");

    // The owner frees the claim, after confirming, with a note for triage.
    await owner.getByRole("button", { name: "Release claim" }).click();
    const confirm = owner.locator("form.af");
    await expect(confirm).toContainText("claim now");
    await confirm.locator("textarea[name=note]").fill("Stuck since Monday");
    await confirm.getByRole("button", { name: "Release claim" }).click();
    await expect(owner.locator(".rh .state")).toHaveText("Open");
    await expect(owner.locator(".cm", { hasText: "Stuck since Monday" }).locator(".only")).toHaveText("Triage only");
    await expect(reply).toHaveValue("Half a reply");
    // The agent's lease is gone: its renew and its fix are lease_lost.
    const renew = await agent.send("POST", `/api/queues/${qid}/reports/${id}/renew`, {});
    expect([renew.status, renew.body.error?.code]).toEqual([409, "lease_lost"]);
    const fix = await agent.send("POST", `/api/queues/${qid}/reports/${id}/fix`, { summary: "x", fence: lease.fence });
    expect([fix.status, fix.body.error?.code]).toEqual([409, "lease_lost"]);
    // The reporter never sees the note.
    const mine = await openReports(emp1);
    await expect(mine).toContainText("The dates are off by one.");
    await expect(mine).not.toContainText("Stuck since Monday");
  } finally {
    await site.stop();
    await d.stop();
  }
});

/** A report filed through the overlay by a site user, signed in as `emp-1`. */
async function fileReport(browser: Browser, site: DeployerSite): Promise<Page> {
  const emp1 = await reporterOn(browser, site, "emp-1");
  await emp1.locator(".report-btn").click();
  await expect(emp1.getByText("Click what your report is about")).toBeVisible();
  await emp1.locator("#req-1 h2").click();
  const composer = emp1.getByRole("form", { name: "New report" });
  await composer.getByRole("textbox", { name: "Report" }).fill("The dates are off by one.");
  await composer.getByRole("button", { name: "Send" }).click();
  await expect(emp1.getByRole("status", { name: "Filed" })).toBeVisible();
  return emp1;
}

/** Report `tid`'s waiting fix made a run's held fix on the daemon of `home`,
 * its preview as `preview` says. A run proposes a fix only through a container,
 * which no daemon here has, so the row is written as a run's would be; all the
 * views then show of it is the daemon's own answer. */
async function holdFix(home: string, tid: string, preview: { url?: string; missed?: boolean } = {}): Promise<void> {
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(join(home, "clax.db"));
  try {
    db.exec("PRAGMA busy_timeout = 5000");
    db.prepare("UPDATE report_fixes SET held = 1, proposal_key = 'e2e', preview_url = ?, preview_missed = ? WHERE thread_id = ? AND outcome = 'pending'")
      .run(preview.url ?? null, preview.missed ? 1 : 0, tid);
  } finally { db.close(); }
}

// On a local daemon: a hosted one takes no model address but a public https
// one, so it cannot reach the scripted Messages API.
test("a report goes to ClaxBot from its view, and the run panel follows it and finds it again", async ({ browser }) => {
  test.setTimeout(90_000);
  const fake = await fakeMessages([finishTurn("The dates come from the server's clock.")]);
  const d = await startDaemon({
    config: `${NO_KEY_CONFIG}[bot]\nbase_url = "${fake.base}"\napi_key_env = "CLAX_E2E_BOT_KEY"\n`,
    env: { CLAX_E2E_BOT_KEY: "sk-ant-e2e-not-a-real-key-0000" },
  });
  const site = await startDeployerSite(d.base);
  try {
    const owner = await (await browser.newContext()).newPage();
    const qid = await queueWithGrant(owner, d.base, site);
    await fileReport(browser, site);

    await owner.goto(`${d.base}/queues/${qid}`);
    await owner.locator("li.rrow a").click();
    const bot = owner.getByRole("region", { name: "ClaxBot" });
    await expect(bot).toContainText("ClaxBot can work on this report, using your credits");
    await bot.getByRole("button", { name: "Send to ClaxBot" }).click();
    const panel = owner.getByRole("complementary", { name: "ClaxBot" });
    await expect(panel.locator(".st")).toHaveText("Finished");
    await expect(panel.locator(".answers")).toContainText("The dates are off by one.");
    await expect(panel.locator(".finish")).toContainText("The dates come from the server's clock.");
    expect(fake.left()).toBe(0);
    await panel.getByRole("button", { name: "Close the run panel" }).click();
    await expect(panel).toHaveCount(0);
    // A reload finds the run on the report, a click away.
    await owner.reload();
    await expect(bot).toContainText("Last run: Finished");
    await bot.getByRole("button", { name: "See the run" }).click();
    await expect(panel.locator(".st")).toHaveText("Finished");
  } finally {
    await site.stop();
    await d.stop();
    await fake.stop();
  }
});

test("a private link reaches ready through its panel and the owner's approval, and a held fix tells members", async ({ browser }, info) => {
  test.setTimeout(90_000);
  const { d, origin } = await hostedDaemon();
  const site = await startDeployerSite(origin);
  try {
    const owner = await ownerOf(browser, origin);
    const qid = await queueWithGrant(owner, origin, site);
    const agent = await agentOf(origin, await agentTokenOf(owner));

    // The queue's admin connects it: each screen says what is still needed.
    await owner.goto(`${origin}/queues`);
    await owner.locator(`li.qcard[data-id="${qid}"]`).getByRole("button", { name: "Manage" }).click();
    const repo = owner.getByRole("region", { name: "Repository of Leave app reports" });
    await repo.getByLabel("Repository", { exact: true }).fill("acme/leave");
    await repo.getByLabel("Preview URL").fill("https://pr-{pr}.acme.dev");
    await repo.getByRole("button", { name: "Connect" }).click();
    await expect(repo.locator(".stat")).toContainText("Not ready: add a write token and a read token below.");
    const setToken = async (which: string, value: string) => {
      const field = repo.locator(`[data-token="${which}"]`);
      await field.getByRole("button", { name: /^Set/ }).click();
      await field.locator('input[type="password"]').fill(value);
      await field.getByRole("button", { name: "Save" }).click();
      await expect(field.getByText(`Set, ending ${value.slice(-4)}`)).toBeVisible();
    };
    await setToken("write", "github_pat_e2e_0123456789_wxyz");
    await expect(repo.locator(".stat")).toContainText("Not ready: add a read token below.");
    await setToken("read", "github_pat_e2e_9876543210_abcd");
    await expect(repo.locator(".stat")).toContainText("this repository's CI needs your say-so below");
    await expect(repo.locator("[data-ci]")).toContainText("before anyone has read the change");
    await expect(repo.locator("[data-allowlist]")).toContainText("such as github.com");
    await repo.getByLabel("This repository's CI may run ClaxBot's branches").check();
    await repo.locator('form.rform button[type="submit"]').click();
    // Ready as a link; this daemon has no container runtime, and the panel says so.
    await expect(repo.locator(".stat")).toContainText("Nothing can use them until this Clax has a container runtime.");

    // The owner asks to approve token links. Their own link comes approved:
    // they withdraw it, read what it will do, and approve it.
    await owner.goto(`${origin}/admin?tab=repos`);
    await owner.getByRole("switch", { name: "Token links need my approval" }).click();
    const line = owner.locator(`[data-link="${qid}"]`);
    await line.getByRole("button", { name: "Withdraw" }).click();
    await expect(line).toContainText("waiting for approval");
    await expect(line.locator("[data-risk]")).toHaveText(["CI may run ClaxBot's branch, with its secrets and network, before anyone reviews it."]);
    await expect(line.locator("[data-terms]")).toContainText("Preview https://pr-{pr}.acme.dev, ready at /healthz");
    await owner.screenshot({ path: shotPath(info, "queues-approval"), fullPage: true });
    await line.getByRole("button", { name: "Approve" }).click();
    await expect(line.getByRole("button", { name: "Withdraw" })).toBeVisible();
    await expect(line).toContainText("· ready");

    const emp1 = await fileReport(browser, site);
    await owner.goto(`${origin}/queues/${qid}`);
    await owner.locator("li.rrow a").click();
    await expect(owner.locator(".rh .state")).toHaveText("Open");

    // A fix is proposed, and a person starts on a fix of their own.
    const id = await owner.evaluate(() => location.pathname.split("/").at(-1)!);
    const lease = await agent.ok<Lease>("POST", `/api/queues/${qid}/reports/${id}/claim`);
    await agent.ok("POST", `/api/queues/${qid}/reports/${id}/fix`, { summary: "A fix has been proposed.", fence: lease.fence });
    await owner.reload();
    await expect(owner.locator('.fix[data-n="1"]')).toContainText("Waiting for the reporter");
    await owner.locator(".btns").getByRole("button", { name: "Propose fix" }).click();
    await owner.locator('textarea[name="summary"]').fill("Have a look.");

    // Meanwhile the fix turns out held. This view does not know yet, so the
    // daemon is the one to refuse a hand-over with no link, in the form's words.
    const NEEDS_LINK = "Say where the reporter checks it: until this fix has a link, they have nothing to check.";
    const alert = owner.locator('.acts [role="alert"]');
    const submit = owner.locator('form.af button[type="submit"]');
    await holdFix(d.home, id);
    await submit.click();
    await expect(alert).toHaveText(NEEDS_LINK);

    // Held, waiting for its preview: members are told, and the reporter has nothing to give a verdict on.
    await owner.reload();
    const held = owner.locator("[data-held]");
    await expect(held).toContainText("The reporter is waiting on you.");
    await expect(held).toContainText("Fix 1 is waiting for its preview, so they have nothing to check yet.");
    await expect(owner.locator('.fix[data-n="1"]')).toContainText("Held for you");
    await owner.screenshot({ path: shotPath(info, "queues-held-fix"), fullPage: true });

    // The wait ends with none.
    await holdFix(d.home, id, { missed: true });
    await owner.reload();
    await expect(held).toContainText("ClaxBot stopped watching for a preview of fix 1 without seeing one up, so they have no link to check.");

    // The preview answers, for members alone: what the daemon sends the reporter holds none of it.
    const PREVIEW = "https://pr-9.acme.dev/";
    await holdFix(d.home, id, { url: PREVIEW });
    await owner.reload();
    await expect(held).toContainText("Fix 1 has a preview, which only this queue's people can see");
    await expect(held.getByRole("link", { name: "check it" })).toHaveAttribute("href", PREVIEW);
    await owner.screenshot({ path: shotPath(info, "queues-held-preview"), fullPage: true });
    const answer = emp1.waitForResponse(r => r.request().method() === "GET" && /\/reports\b/.test(new URL(r.url()).pathname) && r.ok());
    const told = await openReports(emp1);
    const served = await (await answer).text();
    expect(served).toContain("A fix has been proposed.");
    expect(served).not.toContain(PREVIEW);
    expect(served).not.toContain("preview_url");
    expect(served).not.toContain("preview_missed");
    await told.getByRole("button", { name: /The dates are off by one/ }).click();
    const waiting = told.getByRole("region", { name: "Fix waiting for your check" });
    await expect(waiting).toContainText("A fix has been proposed.");
    await expect(waiting.getByRole("link")).toHaveCount(0);
    await expect(waiting.getByRole("button", { name: "Still broken" })).toHaveCount(0);

    // Handing it over starts from the preview; the form takes no hand-over without a link.
    await held.getByRole("button", { name: "Hand it to the reporter" }).click();
    await expect(owner.locator('textarea[name="summary"]')).toHaveValue("A fix is ready for you to check.");
    const link = owner.locator('input[name="verify_url"]');
    await expect(link).toHaveValue(PREVIEW);
    await link.fill("");
    await submit.click();
    await expect(alert).toHaveText(NEEDS_LINK);
    await link.fill(PREVIEW);
    await submit.click();
    await expect(held).toHaveCount(0);
    await expect(owner.locator('.fix[data-n="2"]')).toContainText("Waiting for the reporter");

    // Now the reporter has it, and can say.
    const handed = (await openReports(emp1)).getByRole("region", { name: "Fix waiting for your check" });
    await expect(handed.getByRole("link", { name: "Check it here" })).toHaveAttribute("href", PREVIEW);
    await expect(handed.getByRole("button", { name: "Still broken" })).toBeVisible();
  } finally {
    await site.stop();
    await d.stop();
  }
});
