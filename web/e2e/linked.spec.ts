// Linked threads in browsers (crates/clax-server/src/linked.rs): a local
// daemon signed in to a hosted one by device code pushes an artifact; a
// comment the hosted owner sends to the agent on the public copy comes down
// to the local shell tagged with the remote's name; a reply made in the
// local shell appears on the hosted page; with the hosted daemon gone, a
// local reply reads "Sending to …" and, once its row is marked not sent,
// "Not sent: … unreachable" with Retry; the Publish popover's link row
// shows the comments mode and the outage. A local agent's question about
// the linked artifact is asked on the hosted copy too: it shows on the
// hosted artifact page and inbox, is answered there, and the local `ask`
// returns that answer while the local card says where it was answered,
// tagged with the remote's name. With CLAX_E2E_SHOTS set it writes
// screenshots at 1280 and 390 wide, light and dark, to target/linked-shots/.
import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { type Page } from "@playwright/test";
import { NO_KEY_CONFIG, api, expect, publish, registerSession, startDaemon, test } from "./fixtures";
import { ask, waitAnswer } from "./questions-helpers";

const SECRET = "e2e setup secret";
const SHOTS = !!process.env.CLAX_E2E_SHOTS;
const out = fileURLToPath(new URL("../../target/linked-shots/", import.meta.url));
const SIZES = { desktop: { width: 1280, height: 860 }, phone: { width: 390, height: 844 } } as const;

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

/** Waits until `f` answers something truthy, for up to `ms`. */
async function until<T>(what: string, f: () => Promise<T | null | undefined | false>, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f().catch(() => null);
    if (v) return v;
    if (Date.now() > end) throw new Error(`never: ${what}`);
    await new Promise(r => setTimeout(r, 200));
  }
}

/** Photographs `page` in both themes at both sizes, `prep` run after each change. */
async function shoot(page: Page, name: string, prep: (phone: boolean) => Promise<void>) {
  if (!SHOTS) return;
  mkdirSync(out, { recursive: true });
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of Object.entries(SIZES)) {
      await page.setViewportSize(viewport);
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await prep(size === "phone");
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({ path: join(out, `${name}-${theme}-${size === "phone" ? 390 : 1280}.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
    }
  }
  await page.setViewportSize(SIZES.desktop);
  await page.emulateMedia({ colorScheme: "light" });
}

/** The phone's Threads panel, when the sidebar is not shown beside the page. */
async function threads(page: Page) {
  if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
}

test("a public comment comes down tagged, a local reply goes up, and an outage shows pending then not sent", async ({ browser }) => {
  test.setTimeout(180_000);
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const hosted = await startDaemon({ config: `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`, port, env: { CLAX_E2E_SETUP_SECRET: SECRET }, home: undefined });
  test.skip(hosted.port !== port, "the hosted daemon took another port");
  const local = await startDaemon();
  let hostedUp = true;
  try {
    // The hosted owner sets up.
    const owner = await (await browser.newContext()).newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);

    // The local daemon signs in there by device code; the owner approves.
    const added = await api(local.base, local.token, "/api/remotes", { method: "POST", body: JSON.stringify({ url: origin }) }) as { remote: { id: string }; user_code: string };
    const approved = await owner.evaluate(async uc => (await fetch(`/api/hosted/device/${uc}/approve`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" })).status, added.user_code);
    expect(approved).toBe(200);
    await until("the sign-in connects", async () => {
      const r = await api(local.base, local.token, `/api/remotes/${added.remote.id}/poll`, { method: "POST", body: "{}" }) as { status: string };
      return r.status === "connected";
    }, 30_000);

    // A local artifact, pushed.
    const { artifact } = await publish(local.base, local.token, "Launch plan", { "index.html": "<title>Launch plan</title><h1>Launch plan</h1><p id=\"x\">We ship on Friday.</p>" });
    const pushed = await api(local.base, local.token, `/api/artifacts/${artifact.id}/push`, { method: "POST", body: "{}" }) as { link: { remote_artifact_id: string } };
    const raid = pushed.link.remote_artifact_id;
    const host = `localhost:${port}`;

    // The hosted owner comments on the public copy and sends it to the agent.
    await owner.goto(`${origin}/a/${raid}`);
    const posted = await owner.evaluate(async id => {
      const form = new FormData();
      form.set("anchor", JSON.stringify({ kind: "element", selector: "#x", quote: "We ship on Friday.", file: "index.html" }));
      form.set("body", "@agent Say which Friday: the 17th or the 24th?");
      form.set("version", "1");
      const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
      return { status: r.status, id: (await r.json()).thread?.id as string };
    }, raid);
    expect(posted.status).toBe(201);

    // It comes down to the local shell, tagged with the remote's name.
    const page = await (await browser.newContext()).newPage();
    await page.setViewportSize(SIZES.desktop);
    await page.goto(`${local.base}/a/${artifact.id}`);
    const card = page.locator(".thread-card").filter({ hasText: "which Friday" });
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card.locator(".link-tag").first()).toHaveText(host);
    await expect(card.locator(".msg .author").first()).toContainText("Ada");
    await shoot(page, "mirrored", async phone => { if (phone) { await page.reload(); await threads(page); } await card.waitFor(); });

    // A reply made here appears on the hosted page.
    await page.reload();
    await threads(page);
    await card.locator("input[aria-label=Reply]").fill("The 24th. I have updated the page.");
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    await until("the reply reaches the public thread", async () => {
      const r = await owner.evaluate(async id => (await (await fetch(`/api/artifacts/${id}/threads`)).json()), raid) as { threads: { comments: { body: string }[] }[] };
      return r.threads[0]?.comments.some(c => c.body.startsWith("The 24th"));
    });
    await owner.reload();
    const there = owner.locator(".thread-card").filter({ hasText: "which Friday" });
    if (!(await owner.locator("aside.sidebar").isVisible())) await threads(owner);
    await expect(there).toContainText("The 24th. I have updated the page.");
    await shoot(owner, "hosted-reply", async phone => { await owner.reload(); if (phone) await threads(owner); await there.waitFor(); });

    // The remote goes away: a reply here waits, "Sending to …".
    await hosted.stop();
    hostedUp = false;
    await page.reload();
    await threads(page);
    await card.locator("input[aria-label=Reply]").fill("Also: the launch email goes out at 9.");
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    // Queued here at once, whatever the remote does.
    await until("the reply is queued", async () => {
      const v = await api(local.base, local.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { comments: { body: string; linked?: { state: string | null } }[] }[] };
      return v.threads[0]?.comments.some(c => c.body.startsWith("Also") && c.linked?.state === "pending");
    }, 40_000);
    await expect(card.locator(".link-state > span")).toHaveText(`Sending to ${host}…`);
    await shoot(page, "pending", async phone => { await page.reload(); if (phone) await threads(page); await card.locator(".link-state").waitFor(); });

    // Past the backoff's cap its row is marked not sent (here, on the scratch
    // home's database, rather than waiting five minutes): Retry shows.
    const db = join(local.home, "clax.db");
    const r = spawnSync("sqlite3", [db, "UPDATE linked_outbox SET failed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), last_error = 'Not sent: unreachable'"]);
    expect(r.status, String(r.stderr)).toBe(0);
    await page.reload();
    await threads(page);
    // Said once, under the comment not sent, with Retry beside it.
    await expect(card.locator(".link-state > span")).toHaveText(`Not sent: ${host} unreachable`);
    await expect(card.getByText(`Not sent: ${host} unreachable`)).toHaveCount(1);
    await expect(card.locator(".link-state").getByRole("button", { name: "Retry" })).toBeVisible();
    await shoot(page, "failed", async phone => { await page.reload(); if (phone) await threads(page); await card.getByRole("button", { name: "Retry" }).waitFor(); });

    // The link row: the comments mode, the outage and what was not sent.
    const openLink = async (phone: boolean) => {
      await page.reload();
      if (phone) {
        await page.getByRole("button", { name: /More/ }).first().click();
        await page.getByRole("menuitem", { name: /Link to/ }).click();
      } else {
        await page.getByRole("button", { name: /^Linked/ }).click();
      }
      await page.getByRole("dialog").getByText(`Comments with ${host}`).waitFor();
    };
    await openLink(false);
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("button", { name: "Two-way" })).toHaveAttribute("aria-pressed", "true");
    await expect(dialog).toContainText(`${host} unreachable since`);
    await expect(dialog).toContainText("Not sent: 1 comment");
    await shoot(page, "link-row", openLink);
    await dialog.getByRole("button", { name: "Download only" }).click();
    await expect(page.getByRole("dialog").getByRole("button", { name: "Download only" })).toHaveAttribute("aria-pressed", "true");

    // Retry on the card: the comment reads as sending again at once, and
    // still does when read afresh, with the remote still away.
    await page.reload();
    await threads(page);
    await card.locator(".link-state").getByRole("button", { name: "Retry" }).click();
    await expect(card.locator(".link-state > span")).toHaveText(`Sending to ${host}…`);
    await expect(card.getByRole("button", { name: "Retry" })).toHaveCount(0);
    await page.reload();
    await threads(page);
    await expect(card.locator(".link-state > span")).toHaveText(`Sending to ${host}…`);
  } finally {
    if (hostedUp) await hosted.stop();
    await local.stop();
  }
});

test("a local agent's question is asked on the hosted copy, answered there, and the local card resolves tagged", async ({ browser }) => {
  test.setTimeout(180_000);
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const hosted = await startDaemon({ config: `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`, port, env: { CLAX_E2E_SETUP_SECRET: SECRET }, home: undefined });
  test.skip(hosted.port !== port, "the hosted daemon took another port");
  const local = await startDaemon();
  let hostedUp = true;
  try {
    const owner = await (await browser.newContext()).newPage();
    await owner.setViewportSize(SIZES.desktop);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const added = await api(local.base, local.token, "/api/remotes", { method: "POST", body: JSON.stringify({ url: origin }) }) as { remote: { id: string }; user_code: string };
    const approved = await owner.evaluate(async uc => (await fetch(`/api/hosted/device/${uc}/approve`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" })).status, added.user_code);
    expect(approved).toBe(200);
    await until("the sign-in connects", async () => {
      const r = await api(local.base, local.token, `/api/remotes/${added.remote.id}/poll`, { method: "POST", body: "{}" }) as { status: string };
      return r.status === "connected";
    }, 30_000);
    const { artifact } = await publish(local.base, local.token, "Launch plan", { "index.html": "<title>Launch plan</title><h1>Launch plan</h1><p id=\"x\">We ship on Friday.</p>" });
    const pushed = await api(local.base, local.token, `/api/artifacts/${artifact.id}/push`, { method: "POST", body: "{}" }) as { link: { remote_artifact_id: string } };
    const raid = pushed.link.remote_artifact_id;
    const host = `localhost:${port}`;

    // The local agent, watching the linked artifact, asks about it.
    const s = await registerSession(local.base, local.token);
    await api(local.base, local.token, `/api/sessions/${s.id}/watches/${artifact.id}`, { method: "PUT", body: "{}", session: s.id });
    const asked = await ask(local.base, local.token, s.id, { source: "ask", artifact_id: artifact.id, questions: [
      { question: "Which Friday should the launch go out on?", header: "Launch day", other: true, options: [{ label: "The 17th", description: "Before the conference" }, { label: "The 24th", description: "After the review" }] }] });
    const qid = asked.question.id;

    // The local card carries the remote's tag; once acknowledged, no pending line.
    const page = await (await browser.newContext()).newPage();
    await page.setViewportSize(SIZES.desktop);
    await until("the copy is acknowledged here", async () => {
      const v = await api(local.base, local.token, `/api/questions/${qid}`) as { question: { linked: { sent: boolean } | null } };
      return v.question.linked?.sent;
    });
    await page.goto(`${local.base}/a/${artifact.id}`);
    const local_card = page.locator(`.qcard[data-question="${qid}"]`).first();
    await expect(local_card).toBeVisible({ timeout: 20_000 });
    await expect(local_card.locator(".link-tag")).toHaveText(host);
    await expect(local_card.locator(".link-tag")).toHaveAttribute("title", `Also asked on ${host}`);
    await shoot(page, "questions-local-open", async phone => { await page.reload(); if (phone) await threads(page); await local_card.waitFor(); });

    // Asked on the hosted copy: on its artifact page and in its inbox.
    const rq = await until("the question is asked there", async () => {
      const v = await owner.evaluate(async () => (await (await fetch("/api/questions?status=all")).json())) as { questions: { id: string; artifact: { id: string } | null; questions: { question: string }[] }[] };
      return v.questions.find(q => q.questions[0].question.startsWith("Which Friday"));
    });
    expect(rq.artifact?.id).toBe(raid);
    await owner.goto(`${origin}/inbox`);
    const inboxCard = owner.locator(`.qcard[data-question="${rq.id}"]`).first();
    const inboxRow = owner.locator(".irow", { hasText: "Which Friday" }).first();
    await expect(inboxCard.or(inboxRow)).toBeVisible({ timeout: 20_000 });
    await shoot(owner, "questions-hosted-inbox", async () => { await owner.reload(); await inboxCard.or(inboxRow).waitFor(); });
    await owner.goto(`${origin}/a/${raid}`);
    const there = owner.locator(`.qcard[data-question="${rq.id}"]`).first();
    if (!(await there.isVisible().catch(() => false))) await threads(owner);
    await expect(there).toBeVisible({ timeout: 20_000 });
    // There the local agent is named, with whose Clax it relays from: on the
    // card, in the top bar's roster and in the Send menu.
    await expect(there.locator(".asker")).toHaveText(/^\S+ · via Ada's Clax$/);
    const relayed = (await there.locator(".asker").textContent())!;
    await expect(owner.locator(`.who .tok.a[title="${relayed}"]`)).toHaveCount(1);
    await expect(owner.locator(".who .tok.a[title^='clax-sync']")).toHaveCount(0);
    await shoot(owner, "questions-hosted-artifact", async phone => { await owner.reload(); if (phone) await threads(owner); await there.waitFor(); });

    // Answered there: the local `ask` returns that answer.
    await page.setViewportSize(SIZES.desktop);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto(`${local.base}/a/${artifact.id}`);
    await local_card.waitFor();
    await owner.reload();
    if (!(await there.isVisible().catch(() => false))) await threads(owner);
    await there.locator('input[value="The 24th"]').click();
    await there.locator("button.primary").click();
    await expect(there.locator(".closed")).toHaveText("Answered");
    const answer = await waitAnswer(local.base, local.token, s.id, qid);
    expect(answer.status).toBe("answered");
    expect(answer.answers).toEqual([{ selected: ["The 24th"], text: null }]);

    // The local card, open on the artifact page meanwhile, resolves saying
    // where, with its tag (the shell's clock holds it there until a test
    // advances it); the inbox's read row carries the tag too.
    await expect(local_card.locator(".closed")).toHaveText(`Answered on ${host}`, { timeout: 20_000 });
    await expect(local_card.locator(".link-tag")).toHaveText(host);
    await expect(local_card).toContainText("The 24th");
    await shoot(page, "questions-local-answered", async phone => { if (phone) await threads(page); await local_card.locator(".closed").waitFor(); });
    await page.goto(`${local.base}/inbox`);
    await page.getByRole("button", { name: /^Show \d+ read item/ }).click();
    const row = page.getByRole("button", { name: /^Question: claude asks: Launch day/ });
    await expect(row.locator(".link-tag")).toHaveText(host);
    await shoot(page, "questions-local-inbox", async () => { await page.goto(`${local.base}/inbox`); await page.getByRole("button", { name: /^Show \d+ read item/ }).click(); await row.waitFor(); });

    // With the hosted daemon gone, a question's copy is pending; past the
    // backoff's cap it is not sent (marked here on the scratch home's
    // database), and the link row counts it beside its Retry though no
    // comment failed.
    await hosted.stop();
    hostedUp = false;
    const late = await ask(local.base, local.token, s.id, { source: "ask", artifact_id: artifact.id, questions: [
      { question: "Should the launch email go out at 9?", header: "Email", options: [{ label: "Yes" }, { label: "No" }] }] });
    await until("the copy is pending", async () => {
      const v = await api(local.base, local.token, `/api/questions/${late.question.id}`) as { question: { linked: { state: string | null } | null } };
      return v.question.linked?.state === "pending";
    });
    const r = spawnSync("sqlite3", [join(local.home, "clax.db"), `UPDATE linked_questions SET failed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), last_error = 'Not sent: unreachable' WHERE question_id = '${late.question.id}'`]);
    expect(r.status, String(r.stderr)).toBe(0);
    const openLink = async (phone: boolean) => {
      await page.goto(`${local.base}/a/${artifact.id}`);
      if (phone) {
        await page.getByRole("button", { name: /More/ }).first().click();
        await page.getByRole("menuitem", { name: /Link to/ }).click();
      } else {
        await page.getByRole("button", { name: /^Linked/ }).click();
      }
      await page.getByRole("dialog").getByText(`Comments with ${host}`).waitFor();
    };
    await openLink(false);
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Not sent: 1 question");
    await expect(dialog.getByRole("button", { name: "Retry" })).toBeVisible();
    await shoot(page, "questions-link-row", openLink);
  } finally {
    if (hostedUp) await hosted.stop();
    await local.stop();
  }
});
