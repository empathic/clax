// The hosted demo (demo/), end to end on a native daemon in hosted mode with
// one live people seat: demo/lib/seed.ts seeds it through the browser's
// routes, and demo/lib/agents.ts runs three scripted agents over the MCP
// adapter with the owner's agent token, each drafting live before it
// publishes. Rae's and Mia's browsers, on the
// brief, see the agents' versions land; a comment Rae sends to an agent is
// live in both, and the agent's revision lands in both; Mia's comment sent
// to Rae's agent is held from it. Then the spectator path: a guest takes the
// one seat, Jo (whose share is on another page) opens the brief as a
// read-only spectator, and once the guest leaves Jo joins live.
import { createServer } from "node:net";
import type { BrowserContext, Page } from "@playwright/test";
import { runDemo } from "../../demo/lib/agents";
import { acceptInvite, type PersonSession } from "../../demo/lib/http";
import { HEADING_ANCHOR, seed } from "../../demo/lib/seed";
import { NO_KEY_CONFIG, expect, setName, startDaemon, test } from "./fixtures";

const SECRET = "e2e demo secret";
const BANNER = "Lots of people are here, so you're viewing: comments and live activity are paused.";

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

/** A browser signed in as `who`, by the session cookie seed made for them. */
async function signedIn(ctx: BrowserContext, origin: string, who: PersonSession): Promise<Page> {
  await ctx.addCookies([{ ...who.cookiePair(), url: origin }]);
  return ctx.newPage();
}

/** Shows the newest version on `p`, as a person does: the top bar offers
 * an agent's new version as Reload. */
async function showLatest(p: Page) {
  const reload = p.getByRole("button", { name: "Reload" });
  await expect(reload).toBeVisible({ timeout: 20_000 });
  await reload.click();
  await expect(reload).toHaveCount(0);
}

/** `p` photographed to `target/hosted-shots/demo-<name>.png` when
 * CLAX_E2E_SHOTS is set (for looking at, not compared). */
async function shot(p: Page, name: string) {
  if (process.env.CLAX_E2E_SHOTS) await p.screenshot({ path: `../target/hosted-shots/demo-${name}.png` });
}

/** The thread card on `p` whose text has `text`, with the threads panel open. */
async function card(p: Page, text: string) {
  if (!(await p.locator("aside.sidebar").isVisible())) await p.getByRole("button", { name: /Threads/ }).first().click();
  return p.locator(".thread-card").filter({ hasText: text });
}

test("the seeded demo: agents' versions and a live comment in two people's browsers, then a spectator with one seat", async ({ browser }) => {
  test.setTimeout(120_000);
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const d = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET, CLAX_LIVE_PEOPLE_SEATS: "1" } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}: its public origin would not match`);
  try {
    const s = await seed({ url: origin, secret: SECRET, agents: 3 });
    const brief = s.brief.id;
    const rae = await signedIn(await browser.newContext(), origin, s.owner.session);
    const mia = await signedIn(await browser.newContext({ colorScheme: "dark" }), origin, s.editor.session);
    for (const p of [rae, mia]) {
      await p.goto(`${origin}/a/${brief}`);
      await expect(p.frameLocator("iframe.frame").locator("h1")).toHaveText("Product brief: Trailhead");
    }

    let listening!: () => void;
    const ready = new Promise<void>((r) => { listening = r; });
    const log: string[] = [];
    const demo = runDemo({
      url: origin, token: s.token.token, artifactId: brief, agents: 3, paceMs: 300, listenS: 60, stopAfter: 1,
      log: (l) => log.push(l), onListening: () => listening(),
    });
    // The agents' last drafts land in both browsers, each round's conflicts
    // merged: a version an agent publishes is offered as Reload.
    await ready;
    for (const p of [rae, mia]) await showLatest(p);
    // The agents show as their names (the MCP clients'), not as harnesses;
    // past two working, the summary names one and counts the rest.
    for (const p of [rae, mia]) {
      const who = p.locator(".who .sum b");
      await expect(who).toContainText(/\w+-writer( and \d+ more)? working/);
      await expect(who).not.toContainText(/claude/);
    }
    await shot(rae, "rae-agents");
    if (process.env.CLAX_E2E_SHOTS) {
      // At phone width the names are in the people panel.
      const phone = await mia.context().newPage();
      await phone.setViewportSize({ width: 390, height: 844 });
      await phone.goto(`${origin}/a/${brief}`);
      await phone.getByRole("button", { name: "People and agents" }).click();
      await expect(phone.getByRole("dialog")).toContainText("summary-writer");
      await shot(phone, "mia-agents-phone");
      await phone.close();
    }
    for (const p of [rae, mia]) {
      const frame = p.frameLocator("iframe.frame");
      await expect(frame.locator("#summary")).toContainText("Ask: two engineers and one designer for one quarter.");
      await expect(frame.locator("#users")).toContainText("often with no signal for hours");
      await expect(frame.locator("#proposal")).toContainText("shades each grid cell");
    }

    // The agents' handles, by what each says it is doing.
    const { working } = await s.owner.session.call<{ working: { agent: string; message: string }[] }>("GET", `/api/artifacts/${brief}/working`);
    const handle = (heading: string) => working.find((w) => w.message === `waiting for comments on ${heading}`)!.agent;
    const head = (await s.owner.session.call<{ artifact: { current_version: number } }>("GET", `/api/artifacts/${brief}`)).artifact.current_version;
    // Mia's comment, and Rae's: both live in both browsers, open, before
    // they are sent (the agent answering Rae's resolves it).
    const held = await s.editor.session.form<{ thread: { id: string } }>(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Add a section on pricing.", version: String(head) });
    const asked = await s.owner.session.form<{ thread: { id: string } }>(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Say how many searchers a typical callout has.", version: String(head) });
    for (const p of [rae, mia]) {
      await expect(await card(p, "Say how many searchers")).toBeVisible();
      await expect(await card(p, "Add a section on pricing.")).toBeVisible();
    }
    // Mia's to Rae's agent is held from it until Rae approves Mia; Rae's
    // goes to users-writer, which answers it.
    await s.editor.session.call("POST", `/api/artifacts/${brief}/threads/${held.thread.id}/send`, { to: handle("summary") });
    await s.owner.session.call("POST", `/api/artifacts/${brief}/threads/${asked.thread.id}/send`, { to: handle("who it is for") });
    const r = await demo;
    expect(r.answered.map((a) => a.agent)).toEqual(["users-writer"]);
    expect(r.held).toContainEqual(expect.objectContaining({ agent: "summary-writer", thread_id: held.thread.id, authors: ["Mia"] }));
    expect(r.conflicts.length).toBeGreaterThan(0);
    // Every agent drafted live through the adapter's draft tool, none refused.
    expect([...new Set(r.drafts.map((x) => x.agent))].sort()).toEqual(["proposal-writer", "summary-writer", "users-writer"]);
    expect(log.filter((l) => /no draft tool|draft not shown/.test(l))).toEqual([]);
    expect(log.some((l) => /merged onto v\d+ after a conflict/.test(l))).toBe(true);
    // The revision and the agent's reply land in both.
    for (const p of [rae, mia]) {
      await showLatest(p);
      await expect(p.frameLocator("iframe.frame").locator("#users .revised")).toContainText("Revised for Rae");
      await expect(await card(p, "Say how many searchers")).toContainText("Revised the Who it is for section in version");
    }
    await shot(rae, "rae-answered");
    await shot(mia, "mia-answered");

    // The spectator path: the brief shared by link; a guest takes the one people seat.
    await s.owner.session.call("PUT", `/api/hosted/artifacts/${brief}/link`, { access: "commenter" });
    const guest = await (await browser.newContext()).newPage();
    await guest.goto(`${origin}/a/${brief}`);
    await setName(guest, "Sam");
    await expect(guest.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    await expect.poll(async () => (await (await guest.request.get(`${origin}/api/artifacts/${brief}/snapshot`)).json()).seats_free, { timeout: 20_000 }).toBe(0);
    // Jo, an editor of the checklist only, holds no reserved seat here: a spectator.
    const jo = await signedIn(await browser.newContext(), origin, await acceptInvite(origin, s.invite.url));
    await jo.goto(`${origin}/a/${brief}`);
    const banner = jo.locator(".banner.spectate");
    await expect(banner).toContainText(BANNER, { timeout: 20_000 });
    await expect(jo.getByRole("button", { name: /^Comment/ })).toBeDisabled();
    await shot(jo, "jo-spectator");
    // Rae and Mia stay live meanwhile.
    await expect(rae.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    await expect(mia.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    // The guest leaves: Jo is offered the seat and joins live. The banner
    // shows once Jo's live socket is refused, which may be before her
    // stream asks for the artifact's live topics: a stream that asks after
    // the guest left takes the free seat at once (first come, first served,
    // spec §16.1), and Jo is live with no Join live offered.
    await guest.context().close();
    const join = banner.getByRole("button", { name: "Join live" });
    await expect.poll(async () => (await join.isVisible()) || (await banner.count()) === 0, { timeout: 30_000 }).toBe(true);
    if (await banner.count()) {
      await shot(jo, "jo-join-live");
      await join.click();
    }
    await expect(banner).toHaveCount(0);
    await expect(jo.getByRole("button", { name: /^Comment/ })).toBeEnabled();
    // Live means the artifact's topics reach Jo: a comment Rae posts now
    // shows on Jo's page with no snapshot poll.
    const now = (await s.owner.session.call<{ artifact: { current_version: number } }>("GET", `/api/artifacts/${brief}`)).artifact.current_version;
    await s.owner.session.form(`/api/artifacts/${brief}/threads`, { anchor: HEADING_ANCHOR, body: "Jo, the pricing section is yours.", version: String(now) });
    await expect(await card(jo, "Jo, the pricing section is yours.")).toBeVisible();
  } finally {
    await d.stop();
  }
});
