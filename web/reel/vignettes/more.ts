// "Pages that do more" (spec 2026-10-27-sizzle-reel-design §3): a page with
// real runtime capabilities. On a hosted daemon (a local daemon's browsers
// are all its owner, web/e2e/hosted-helpers.ts), Alex, the owner, watches
// Northwind's standup board while Mia and Sam, members with rights on it,
// post and vote in browsers of their own: Mia's update slides into Alex's
// list and the vote counts bump as their votes land, all through the page's
// shared database (db). The page also declares the internet (net) for
// api.northwind.example: the owner's approval bar asks for it, Alex
// reviews and approves, and the view, reloaded with net, fetches the weekly
// signups through the daemon and draws them. Only Alex's browser is filmed;
// his threads panel stays as the shell opens it at this width, so the
// reload after the approval changes no layout.
//
// The signups never touch a network. The daemon answers a fresh entry of
// its net cache (crates/clax-http/src/net_cache.rs) without resolving or
// connecting, after the app's own host check; the cache is indexed when the
// daemon starts and keyed by the artifact's ID. So the setup publishes the
// page, stops the daemon, writes the answer (fixtures/more/signups.json) as
// the entry for this artifact's GET of the URL, and starts the daemon again
// on the same home and port, before anyone opens the page.
//
// Mia is teal and Sam blue every run (people.ts), both joining before Alex.
// Their steps are scripted in their own frames (the page's form and vote
// buttons, no pointer of theirs moves), and each is waited for in Alex's
// frame (it heard the database change) before the film moves on.
//
// Beats: `live` (Mia's update and the votes arrive), `approve` (Alex
// reviews and approves the internet), `data` (the signups chart fills).
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Frame, Locator, Page } from "@playwright/test";
import { NO_KEY_CONFIG, publishWith } from "../../e2e/fixtures.ts";
import { SECRET, openAt, ownerOf, rightsOn } from "../../e2e/hosted-helpers.ts";
import { quietIn } from "../camera.ts";
import { cursorPress } from "../cursor.ts";
import type { Director, Stage, Vignette } from "../director.ts";
import { memberIn, welcomedColor } from "../people.ts";

const HOST = "api.northwind.example";
const SIGNUPS = `https://${HOST}/v1/signups/weekly`;
const CAPABILITIES = { db: {}, user: { scopes: ["profile"] }, net: { hosts: [HOST] } };

/** The board's updates and votes before anyone arrives. Northwind and its people are invented. */
const UPDATES = [
  { id: "pricing", text: "Pricing page v2 is live, conversion up 6%", by: "Priya", order: 1 },
  { id: "ledger", text: "Billing moves to the new ledger next sprint", by: "Alex", order: 2 },
  { id: "sso", text: "Need a reviewer for the SSO migration", by: "Jon", order: 3 },
];
const VOTES = [
  { update: "pricing", by: "Alex" },
  { update: "pricing", by: "Jon" },
  { update: "ledger", by: "Priya" },
];
const MIAS = "Onboarding checklist ships Friday";
/** Mia's and Sam's live colours (LIVE_PALETTE): teal and blue. */
const MIA = 2;
const SAM = 0;

type Cast = { alex: Page; af: Frame; aid: string; mia: Page; mf: Frame; sam: Page; sf: Frame };

/** The daemon's net cache key (net_cache::key_of) of a header-less GET of `url` by artifact `app`. */
export function cacheKey(app: string, url: string): string {
  const h = createHash("sha256");
  const field = (b: Buffer) => {
    const len = Buffer.alloc(8);
    len.writeBigUInt64LE(BigInt(b.length));
    h.update(len);
    h.update(b);
  };
  field(Buffer.from(app));
  field(Buffer.from(new URL(url).href));
  field(Buffer.alloc(8)); // no headers
  return h.digest("hex");
}

/** Writes `body` as a fresh JSON answer to `url` for artifact `app` into the net cache under `home`. */
function seedNetCache(home: string, app: string, url: string, body: string): void {
  const dir = join(home, "net-cache");
  mkdirSync(dir, { recursive: true });
  const bytes = Buffer.from(body);
  const entry = {
    url: new URL(url).href,
    status: 200,
    headers: { "cache-control": "max-age=315360000", "content-type": "application/json" },
    received: Math.floor(Date.now() / 1000),
    body_len: bytes.length,
  };
  writeFileSync(join(dir, cacheKey(app, url)), Buffer.concat([Buffer.from(`${JSON.stringify(entry)}\n`), bytes]));
}

// What Alex's content frame hears from the shell: how many of the page's
// documents changed (`clax:event` db/changed).
const HEARING = `(() => {
  if (globalThis.reelHeard) return;
  const heard = globalThis.reelHeard = { changed: 0 };
  addEventListener("message", e => {
    const m = e.data;
    if (m && m.type === "clax:event" && m.ns === "db" && m.topic === "changed") heard.changed += 1;
  });
})()`;

const changesHeard = (alex: Page, af: Frame) => quietIn<number>(alex, af, "globalThis.reelHeard.changed");

/** Runs `step` in someone's frame, off camera, and waits until Alex's frame has heard the change it makes. */
async function landed(d: Director, cast: Cast, step: () => Promise<unknown>, what: string): Promise<void> {
  const before = await changesHeard(cast.alex, cast.af);
  await d.offstage(step);
  await d.until(async () => (await changesHeard(cast.alex, cast.af)) > before, what);
}

/** Clicks the vote button on the update reading `text` in `frame`, as the page's own button. */
const voteOn = (frame: Frame, text: string) =>
  frame.evaluate(t => {
    const li = [...document.querySelectorAll("li[data-update]")].find(l => l.querySelector(".text")?.textContent === t);
    if (!li) throw new Error(`no update "${t}"`);
    (li.querySelector("button.vote") as HTMLButtonElement).click();
  }, text);

/** Moves to `target` and presses it (the cursor's press ring, a real
 * press and release), off camera: the film stands while the press lands. */
async function press(d: Director, page: Page, target: Locator): Promise<void> {
  await d.move(target);
  await d.offstage(async () => {
    await cursorPress(page);
    await page.mouse.down();
    await page.mouse.up();
  });
}

export default {
  id: "more",

  async setup(stage: Stage): Promise<Cast> {
    const { d, origin } = await stage.hostedDaemon();
    const html = readFileSync(new URL("../fixtures/more/standup.html", import.meta.url), "utf8");
    const signups = readFileSync(new URL("../fixtures/more/signups.json", import.meta.url), "utf8");
    const { artifact } = await publishWith(d.base, d.token, "Northwind Standup", html, CAPABILITIES);
    const aid = artifact.id;

    // The signups answer goes into the net cache while the daemon is down,
    // and the daemon comes back on the same home and port (its origin).
    await d.stop({ keepHome: true });
    seedNetCache(d.home, aid, SIGNUPS, signups);
    const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
    const again = await stage.startDaemon({ config, port: d.port, home: d.home, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
    if (again.port !== d.port) throw new Error(`the daemon came back on port ${again.port}, not ${d.port}: its public origin would not match`);

    // Alex sets the daemon up, seeds the board and invites Mia and Sam, in
    // a browser of the setup's own, then leaves the page; the camera signs
    // in as him.
    const owner = await ownerOf(stage.browser, origin, "Alex");
    const seed = await openAt(owner, origin, aid, 1);
    await seed.evaluate(async ([updates, votes]) => {
      type Db = { doc(p: string): { set(v: unknown): Promise<unknown> } };
      const db = await (window as unknown as { claude: { use(n: string): Promise<Db> } }).claude.use("db");
      for (const { id, ...u } of updates) await db.doc(`updates/${id}`).set(u);
      for (const v of votes) await db.doc(`votes/${v.update}~${v.by}`).set(v);
    }, [UPDATES, VOTES] as const);
    await owner.goto(`${origin}/`);
    const mia = await memberIn(stage.browser, owner, origin, "Mia", rightsOn(aid), MIA);
    const sam = await memberIn(stage.browser, owner, origin, "Sam", rightsOn(aid), SAM);
    const signedIn = await owner.context().storageState();
    await owner.context().close();

    const frames: Frame[] = [];
    for (const [page, name, color] of [[mia, "Mia", MIA], [sam, "Sam", SAM]] as const) {
      await page.setViewportSize({ width: 1280, height: 800 });
      const welcomed = welcomedColor(page);
      frames.push(await openAt(page, origin, aid, 1));
      const got = await welcomed;
      if (got !== color) throw new Error(`${name} joined in live colour ${got}, not ${color}`);
    }
    const [mf, sf] = frames as [Frame, Frame];
    const alex = await stage.newCamera({ storageState: signedIn });
    const af = await openAt(alex, origin, aid, 1);
    for (const [page, frame] of [[alex, af], [mia, mf], [sam, sf]] as const) {
      await frame.getByText("Need a reviewer for the SSO migration").waitFor({ state: "visible" });
      await page.waitForFunction(() => ((window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0) > 0);
    }
    for (const name of ["Mia", "Sam"]) await alex.locator(`.who .tok.p.here[title="${name}"]`).waitFor({ state: "visible" });
    await alex.locator("[data-approval-bar]").waitFor({ state: "visible" });
    await quietIn(alex, af, HEARING);
    // The bar's Review button eases its hover and press: a transition that
    // starts while the film stands (the press, then the dialog's opening)
    // can end in real time before the camera first sees it, or not. It
    // shows, here only, at once.
    await alex.addStyleTag({ content: "[data-approval-bar] button { transition: none !important; }" });
    return { alex, af, aid, mia, mf, sam, sf };
  },

  async play(d: Director, cast: Cast): Promise<void> {
    const { alex, af, mf, sf } = cast;
    // Alex rests his pointer by the updates.
    await d.hold(300);
    await d.move(af.getByText("Billing moves to the new ledger next sprint"), { offset: { x: 60, y: 30 } });
    await d.hold(200);

    // Mia posts an update, and the votes come in.
    d.beat("live");
    await landed(d, cast, () => mf.evaluate(t => {
      const input = document.getElementById("text") as HTMLInputElement;
      input.value = t;
      (document.getElementById("post") as HTMLFormElement).requestSubmit();
    }, MIAS), "Mia's update reached Alex's board");
    await d.hold(750);
    await landed(d, cast, () => voteOn(sf, MIAS), "Sam's vote on Mia's update reached Alex's board");
    await d.hold(450);
    await landed(d, cast, () => voteOn(mf, "Need a reviewer for the SSO migration"), "Mia's vote on the SSO review reached Alex's board");
    await d.hold(450);
    await landed(d, cast, () => voteOn(sf, "Pricing page v2 is live, conversion up 6%"), "Sam's vote on pricing reached Alex's board");
    await d.hold(550);

    // The page asks for the internet: Alex reviews and approves.
    d.beat("approve");
    // Each press is made with the film stilled, and the film goes on once
    // what it opens is there: the dialog (a chunk the shell loads on first
    // use) and, after Approve, the reloaded view with its signups drawn; so
    // each lands between the same two frames every run.
    await press(d, alex, alex.locator("[data-approval-bar]").getByRole("button", { name: "Review" }));
    const approve = alex.getByRole("dialog").getByRole("button", { name: "Approve", exact: true });
    await d.until(() => approve.isVisible(), "the approval dialog opened");
    await d.hold(600);
    await press(d, alex, approve);

    // The page reloads with net and draws the signups from the API.
    d.beat("data");
    // The frame may be a new one: it is found again by its URL each time.
    const url = new RegExp(`${cast.aid}\\.localhost:\\d+/_clax/v/1/(#.*)?$`);
    const charted = async () => {
      const f = alex.frame({ url });
      return !!f && (await quietIn<boolean>(alex, f, `document.getElementById("chart")?.dataset.ready === "1"`).catch(() => false));
    };
    await d.until(charted, "the signups reached Alex's chart");
    await d.hold(700);
    const nf = alex.frame({ url }) ?? af;
    await d.move(nf.locator("#kpi"), { offset: { x: 20, y: 10 } });
    await d.hold(1800);
  },
} satisfies Vignette<Cast>;
