// A user-story walkthrough of shared report queues, rendered for a person to
// read: screenshots of each step, and beside each the accessible snapshot of
// the region the step is about, into $CLAX_SHOTS_DIR as NN-name.png and
// NN-name.aria.txt (and api-refusals.txt for what a triage member's browser
// is refused). On a hosted daemon that signs people in with a fake GitHub
// (e2e/fake-github.ts): Robert, its owner, has a queue for a deployer's site
// (e2e/deployer-site); he adds Mia, already a person here, as triage from
// the people search; Mia finds the Queues link, the queue and the notice;
// he adds @octo-cat, who has no account yet, sees the row waiting for their
// GitHub sign-in, removes it and adds it again; octo-cat signs in with
// GitHub and lands in the queue with the notice; a user of the site files a
// report through the overlay; Mia reads it, her agent claims it, she replies;
// the reporter reads the reply; and Mia, as triage, has no Manage or Delete.
// Then what octo-cat sees on the queue's reports, and Lee, invited with no
// rights and added as triage, refused them (23, 24).
// Desktop (1280×800), light, reduced motion. Skipped without
// CLAX_SHOTS_DIR; the `shots` project, run on demand only (from web/, with
// CLAX_TEST_BIN a built debug clax, or none to have cargo build it):
//
//   CLAX_TEST_BIN=../target/debug/clax CLAX_SHOTS_DIR=$PWD/walkthrough \
//     pnpm exec playwright test --project=shots --workers=1 e2e/walkthrough-shared-queues.spec.ts
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Browser, BrowserContextOptions, Locator, Page } from "@playwright/test";
import { startDeployerSite } from "./deployer-site/server";
import { startFakeGitHub } from "./fake-github";
import { asStranger, expect, test } from "./fixtures";
import { hostedDaemon, memberOf, ownerOf } from "./hosted-helpers";
import { agentOf, agentTokenOf, openReports, queueWithGrant, reporterOn } from "./queues-helpers";

const OUT = process.env.CLAX_SHOTS_DIR || "";
test.skip(!OUT, "set CLAX_SHOTS_DIR to the directory the walkthrough goes to");

/** The Collaborator preset of Settings › People (web/shell/src/people/model.ts PRESETS). */
const COLLABORATOR = [
  { type: "clax-artifacts", actions: ["read"], locations: ["*"] },
  { type: "clax-comments", actions: ["read", "reply", "watch"], locations: ["*"] },
  { type: "clax-db", actions: ["read"], locations: ["*"], level: "view" },
  { type: "clax-ask", actions: ["ask"] },
  { type: "clax-artifacts", actions: ["create"], locations: ["*"] },
  { type: "clax-artifacts", actions: ["update", "delete"], locations: ["own"] },
  { type: "clax-comments", actions: ["write", "resolve", "send"], locations: ["*"] },
  { type: "clax-db", actions: ["read", "write"], locations: ["*"], level: "interact" },
  { type: "clax-db", actions: ["read", "write"], locations: ["own"], level: "admin" },
];

const VIEW: BrowserContextOptions = { viewport: { width: 1280, height: 800 }, colorScheme: "light", reducedMotion: "reduce" };

/** `browser`, whose every new context is a desktop one, light, with reduced motion. */
const desktop = (browser: Browser): Browser =>
  new Proxy(browser, { get: (b, k) => (k === "newContext" ? (o: BrowserContextOptions = {}) => b.newContext({ ...VIEW, ...o }) : Reflect.get(b, k, b)) });

/** Once `page` has settled (its fonts loaded, its animations finished):
 * its screenshot as `name`.png, and `region`'s accessible snapshot as
 * `name`.aria.txt. */
async function shot(page: Page, name: string, region: Locator) {
  await expect(region).toBeVisible();
  // The shell's loading frame gone (web/shell/src/view/gallery-frame.ts: a busy main, its skeleton).
  await expect(page.locator(".gskel")).toHaveCount(0);
  await expect(page.locator('main[aria-busy="true"]')).toHaveCount(0);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    // A running animation is finished; an endless one (a spinner) cannot be, and stays.
    for (const a of document.getAnimations()) {
      try { a.finish(); } catch { /* endless */ }
    }
  });
  await page.screenshot({ path: join(OUT, `${name}.png`), fullPage: false });
  writeFileSync(join(OUT, `${name}.aria.txt`), `${await region.ariaSnapshot()}\n`);
}

/** `page`'s inbox as it opens (its "Waiting on you" view), shot as
 * `name`-default, then its "All items" view with the notice `notice`
 * shown, shot as `name`-all-items. */
async function inboxNotice(page: Page, name: string, notice: RegExp) {
  await page.goto(new URL("/inbox", page.url()).toString());
  const items = page.getByRole("region", { name: "Items" });
  await expect(items).toBeVisible();
  await shot(page, `${name}-default`, page.locator("main"));
  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "All items" }).click();
  await expect(items.getByText(notice).first()).toBeVisible();
  await shot(page, `${name}-all-items`, page.locator("main"));
}

/** The whole walkthrough's hang guard: some two dozen steps, each a page load and a shot. */
const HANG_MS = 240_000;

test("shared report queues, as a user story", async ({ browser: plain }) => {
  test.setTimeout(HANG_MS);
  mkdirSync(OUT, { recursive: true });
  const browser = desktop(plain);
  const gh = await startFakeGitHub("e2e-client", "e2e-secret");
  const { d, origin } = await hostedDaemon(
    `github_client_id = "e2e-client"\ngithub_client_secret_env = "CLAX_E2E_GITHUB_SECRET"\ngithub_api_base = "${gh.base}"\ngithub_oauth_base = "${gh.base}"\n`,
    { CLAX_E2E_GITHUB_SECRET: "e2e-secret" },
  );
  const site = await startDeployerSite(origin);
  const refusals: string[] = [];
  try {
    // Robert owns this Clax and has a queue for his site; Mia is already a person here.
    const robert = await ownerOf(browser, origin, "Robert");
    robert.on("dialog", dlg => void dlg.accept());
    const qid = await queueWithGrant(robert, origin, site);
    const queue = "Leave app reports";
    // Mia joined from an invite with the Collaborator preset (web/shell/src/people/model.ts).
    const mia = await memberOf(browser, robert, origin, "Mia", COLLABORATOR);

    // 1. Robert opens Queues: his queue, with him its maker.
    await robert.goto(`${origin}/queues`);
    const card = robert.locator("li.qcard", { has: robert.getByRole("heading", { name: queue }) });
    await expect(card.locator(".counts")).toHaveText("0 open · 0 claimed · 0 fix proposed · 0 verified · 0 resolved");
    await shot(robert, "01-queues-list", robert.locator("main"));

    // 2. Manage → Members: he alone, as the queue's maker.
    await card.getByRole("button", { name: "Manage" }).click();
    const members = card.getByRole("region", { name: `Members of ${queue}` }).or(card.locator(`section[aria-label="Members of ${queue}"]`));
    await expect(members.locator("li.line[data-member]").first()).toContainText("made this queue");
    await members.scrollIntoViewIfNeeded();
    await shot(robert, "02-members-before", members);

    // He finds Mia by name and adds her as Triage.
    await card.getByLabel("Add a person").fill("mi");
    const found = card.getByRole("list", { name: "People found" });
    await expect(found.getByRole("button", { name: /Mia/ })).toBeVisible();
    await shot(robert, "03-people-search", members);
    await found.getByRole("button", { name: /Mia/ }).click();
    await card.getByRole("button", { name: "Add member" }).click();
    await expect(members.locator("li.line[data-member]")).toHaveCount(2);
    await expect(members.getByRole("combobox", { name: "Role of Mia" })).toHaveValue("triage");
    await shot(robert, "04-teammate-added", members);

    // 3. Mia: the Queues link, the queue, and the notice of who added her.
    await mia.reload();
    await expect(mia.locator('a.queues-link[href="/queues"]')).toBeVisible();
    await shot(mia, "05-teammate-home-queues-link", mia.locator("body"));
    await mia.locator('a.queues-link[href="/queues"]').click();
    await expect(mia.getByRole("heading", { name: queue })).toBeVisible();
    await expect(mia.locator(".qcard .role").first()).toHaveText("Triage");
    await shot(mia, "06-teammate-queues", mia.locator("main"));
    await inboxNotice(mia, "07-teammate-inbox", new RegExp(`Robert added you to ${queue} as triage`));

    // 4. Robert adds @octo-cat, who has no account here yet: a row waits
    // for their GitHub sign-in; he removes it, then adds it again.
    await card.getByLabel("Add a person").fill("@octo-cat");
    const addLogin = card.getByRole("button", { name: "Add @octo-cat" });
    await expect(addLogin).toBeEnabled();
    await shot(robert, "08-add-by-login-typed", members);
    await addLogin.click();
    const waiting = card.getByRole("list", { name: "Waiting for their GitHub sign-in" });
    await expect(waiting).toContainText("@octo-cat · waiting for their GitHub sign-in");
    await shot(robert, "09-waiting-for-sign-in", members);
    await waiting.getByRole("button", { name: "Remove @octo-cat" }).click();
    await expect(waiting).toHaveCount(0);
    await shot(robert, "10-waiting-row-removed", members);
    await card.getByLabel("Add a person").fill("@octo-cat");
    await card.getByRole("button", { name: "Add @octo-cat" }).click();
    await expect(waiting).toContainText("@octo-cat · waiting for their GitHub sign-in");

    // 5. octo-cat signs in with GitHub (the fake) and lands in the queue, with the notice.
    gh.account(7, "octo-cat", "Octo Cat");
    gh.signInAs(7);
    const catContext = await browser.newContext();
    await asStranger(catContext);
    const cat = await catContext.newPage();
    await cat.goto(`${origin}/auth/github?next=/queues`);
    await expect(cat).toHaveURL(`${origin}/queues`);
    await expect(cat.getByRole("heading", { name: queue })).toBeVisible();
    await shot(cat, "11-invitee-signed-in-queues", cat.locator("main"));
    await inboxNotice(cat, "12-invitee-inbox", new RegExp(`added you to ${queue} as triage`));
    await robert.reload();
    const card2 = robert.locator("li.qcard", { has: robert.getByRole("heading", { name: queue }) });
    await card2.getByRole("button", { name: "Manage" }).click();
    const members2 = card2.locator(`section[aria-label="Members of ${queue}"]`);
    await expect(members2).toContainText("Octo Cat");
    await expect(card2.getByRole("list", { name: "Waiting for their GitHub sign-in" })).toHaveCount(0);
    await members2.scrollIntoViewIfNeeded();
    await shot(robert, "13-members-after-sign-in", members2);

    // 6. A user of the site files a report through the overlay.
    const emp = await reporterOn(browser, site, "emp-1");
    // The whole document: the overlay's host is outside the site's body.
    await shot(emp, "14-site-with-overlay", emp.locator(":root"));
    await emp.locator(".report-btn").click();
    await expect(emp.getByText("Click what your report is about")).toBeVisible();
    await emp.locator("#req-1 h2").click();
    const composer = emp.getByRole("form", { name: "New report" });
    await expect(composer.getByRole("img", { name: "Screenshot of the picked part of the page" })).toBeVisible();
    const body = "The dates read 3 to 7 November but the request was for 4 days: which is right?";
    await composer.getByRole("textbox", { name: "Report" }).fill(body);
    await shot(emp, "15-report-composer", composer);
    await composer.getByRole("button", { name: "Send" }).click();
    const filed = emp.getByRole("status", { name: "Filed" });
    await expect(filed).toBeVisible();
    await shot(emp, "16-report-filed", filed);

    // 7. Mia sees it in the queue, her agent claims it, and she replies.
    await mia.goto(`${origin}/queues/${qid}`);
    const row = mia.locator("li.rrow");
    await expect(row).toHaveCount(1);
    await expect(row.locator(".note")).toHaveText(body);
    await shot(mia, "17-triage-queue-view", mia.locator("main"));
    const id = (await row.getAttribute("data-id"))!;
    await row.locator("a").click();
    await expect(mia.locator(".rh .state")).toHaveText("Open");
    await shot(mia, "18-triage-report-open", mia.locator("main"));
    const agent = await agentOf(origin, await agentTokenOf(mia, "mia-laptop"));
    await agent.ok("POST", `/api/queues/${qid}/reports/${id}/claim`);
    await expect(mia.locator(".rh .state")).toHaveText("Claimed");
    await expect(mia.locator(".by .claim")).toContainText("claimed by");
    await shot(mia, "19-triage-report-claimed", mia.locator("main"));
    const reply = "Thanks: the request is for 3 to 6 November, and the end date is fixed in the next deploy.";
    await mia.locator("#rv-reply").fill(reply);
    await mia.getByRole("button", { name: "Reply", exact: true }).click();
    await expect(mia.locator(".convo .cm .body").last()).toHaveText(reply);
    await expect(mia.locator("#rv-reply")).toHaveValue("");
    await mia.locator(".convo").scrollIntoViewIfNeeded();
    await shot(mia, "20-triage-replied", mia.locator(".convo"));

    // 8. The reporter reads the reply in "Your reports".
    const mine = await openReports(emp);
    const theirs = mine.getByRole("listitem").filter({ hasText: "The dates read 3 to 7 November" });
    await expect(theirs.locator(".state")).toHaveText("Being worked on");
    await theirs.getByRole("button", { name: /The dates read/ }).click();
    await expect(theirs.locator(".c .body").last()).toHaveText(reply);
    await shot(emp, "21-reporter-sees-reply", mine);

    // 9. What Mia, as triage, may not do: no Manage or Delete on her card,
    // and what this browser is refused when it asks anyway.
    await mia.goto(`${origin}/queues`);
    const miaCard = mia.locator("li.qcard", { has: mia.getByRole("heading", { name: queue }) });
    await expect(miaCard.locator(".role").first()).toHaveText("Triage");
    await expect(miaCard.getByRole("button", { name: "Manage" })).toHaveCount(0);
    await expect(miaCard.getByRole("button", { name: `Delete queue ${queue}` })).toHaveCount(0);
    await shot(mia, "22-triage-no-manage-or-delete", miaCard);
    // The delete last: were it let through, the others would still have been asked of the queue.
    for (const [method, path, payload] of [
      ["PUT", `/api/queues/${qid}/members`, { github_login: "someone-else", role: "triage" }],
      ["PATCH", `/api/queues/${qid}`, { name: "Renamed by triage" }],
      ["DELETE", `/api/queues/${qid}`, undefined],
    ] as const) {
      const r = await mia.evaluate(async ([m, p, b]) => {
        const res = await fetch(p, { method: m, ...(b ? { headers: { "content-type": "application/json" }, body: JSON.stringify(b) } : {}) });
        return `${res.status} ${await res.text()}`;
      }, [method, path, payload ?? null] as const);
      refusals.push(`${method} ${path.replace(qid, "<queue>")}${payload ? ` ${JSON.stringify(payload)}` : ""} -> ${r}`);
    }

    // What octo-cat, who came in by GitHub sign-in with no rights set by
    // the owner, sees on the queue's reports; and Lee, a person invited
    // with no rights and added as triage like Mia: whatever each is shown.
    const settledView = (p: Page) => p.locator("li.rrow").first().or(p.getByText(/Could not load the reports|No active reports/));
    await cat.goto(`${origin}/queues/${qid}`);
    await expect(settledView(cat)).toBeVisible();
    await shot(cat, "23-invitee-queue-view", cat.locator("main"));
    const lee = await memberOf(browser, robert, origin, "Lee", []);
    await card2.getByLabel("Add a person").fill("lee");
    await card2.getByRole("list", { name: "People found" }).getByRole("button", { name: /Lee/ }).click();
    await card2.getByRole("button", { name: "Add member" }).click();
    await expect(members2.getByRole("combobox", { name: "Role of Lee" })).toHaveValue("triage");
    await lee.goto(`${origin}/queues/${qid}`);
    await expect(settledView(lee)).toBeVisible();
    await shot(lee, "24-triage-without-comment-rights", lee.locator("main"));
    await catContext.close();
  } finally {
    writeFileSync(join(OUT, "api-refusals.txt"), `${refusals.join("\n")}\n`);
    await site.stop();
    await d.stop();
    await gh.stop();
  }
});
