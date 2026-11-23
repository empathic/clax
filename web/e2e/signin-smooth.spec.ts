// What a person sees on a hosted Clax before and while signing in loads in
// its final shape: the landing page (`/start/`, where `/` sends a
// signed-out visitor), the sign-in pages (`/signin`, `/signin/<code>`,
// `/join/<code>`, `/setup`, `/device`), and the journey through them, from
// a signed-out visit to `/`, through GitHub's sign-in (a fake GitHub here),
// to the gallery and back out. Each load is played one response at a time
// (`load-stages.ts`): before any script or data is let through, the
// document alone must paint the page's frame (the header, and the card
// with its heading, or the landing's header and stage), in the theme the
// page settles in; nothing moves (the Layout Instability API,
// `layout-shift.ts`); the card's heading, its first paragraph, its main
// button and the footer are each written once; and the frame's landmarks
// (the header bar, mark, title, theme switch, the card and its heading)
// are where the settled page puts them, at every stage. A page the server
// knows the person should not see (the sign-in page of a browser already
// signed in, sent on by `?next=`) is never a document of its own: the
// server redirects. Each runs on a desktop, a phone, and a phone with
// touch, in light and in dark.
import { createServer, type Server } from "node:http";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { test, expect, NO_KEY_CONFIG, publishAs, registerSession, startDaemon, type Daemon } from "./fixtures";
import { SECRET, freePort } from "./hosted-helpers";
import { type Landmark, type Place, GALLERY_APART, SIGNIN_APART, formatShifts, overlaps, formatTexts, landmarks, layoutShifts, placesDiffer, recordLayoutShifts, recordTexts, shiftTotal, shownTexts, textHistory } from "./layout-shift";
import { type StageRule, type StagedLoad, stageLoad } from "./load-stages";
import { quietEval } from "./time";

/** The order held requests are released in: the page's code and styles,
 * then the answers in the order the page reads them, then the rest. */
/** The hang guard of a test that loads many pages, each one response at a time. */
const HANG_MS = 120_000;

const ORDER: StageRule[] = [
  { name: "css", match: (_u, r) => r.resourceType() === "stylesheet" },
  { name: "js", match: (_u, r) => r.resourceType() === "script" },
  { name: "font", match: (_u, r) => r.resourceType() === "font" },
  { name: "hosted", match: u => u.pathname === "/api/hosted/me" },
  { name: "peek", match: u => /^\/api\/hosted\/(invites|signin-links|device)\//.test(u.pathname) },
  { name: "public", match: u => u.pathname === "/api/hosted/public" },
  { name: "frame", match: (_u, r) => r.resourceType() === "document" },
  { name: "api", match: u => u.pathname.startsWith("/api/") && !u.pathname.startsWith("/api/stream") },
  { name: "stream", match: u => u.pathname.startsWith("/api/stream") },
];

/** The page's header, `x` in it: the frame's (`.aframe`, which the landing
 * page keeps), else the mounted page's (the gallery's frame's header,
 * beside the sign-in frame, is not shown). */
const BAR = (x = "") => `#app > .aframe > .gbar${x}, #app > .gbar:not(:has(~ .aframe))${x}`;
/** The card, as the frame (`.aframe`) and the mounted page show it. */
const CARD = "#app main.signin > .sicard";
/** The texts each written once: the card's heading, its first paragraph, its main control, the footer. */
const TEXTS = [`${CARD} h2`, `${CARD} > p`, `${CARD} .primary`, "#app main.signin > .silegal", BAR(" .primary")];

/** The landmarks a sign-in page's frame shows, held to the settled page's. */
const SIGNIN_MARKS: Landmark[] = [
  { name: "header bar", skeleton: null, content: BAR() },
  { name: "mark", skeleton: null, content: BAR(" .home .mark") },
  { name: "title", skeleton: null, content: BAR(" h1") },
  { name: "theme switch", skeleton: null, content: BAR(" .theme-switch") },
  { name: "card", skeleton: null, content: CARD, keys: ["x", "y", "w"] },
  { name: "heading", skeleton: null, content: `${CARD} h2` },
  { name: "landing link", skeleton: null, content: BAR(" .primary") },
  { name: "stage", skeleton: null, content: "#app .lframe" },
];

/** A fake GitHub: its authorize page signs `login` in at once, its token
 * and user routes answer for them. */
async function fakeGitHub(login = "octo"): Promise<{ base: string; server: Server }> {
  const port = await freePort();
  const server = createServer((req, res) => {
    const u = new URL(req.url ?? "/", `http://127.0.0.1:${port}`);
    if (u.pathname === "/login/oauth/authorize") {
      const back = new URL(u.searchParams.get("redirect_uri")!);
      back.searchParams.set("code", "e2e-code");
      back.searchParams.set("state", u.searchParams.get("state")!);
      res.writeHead(302, { location: back.href }).end();
    } else if (u.pathname === "/login/oauth/access_token") {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ access_token: "e2e-token" }));
    } else if (u.pathname === "/user") {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ id: 4242, login, name: "Octo Cat", avatar_url: null }));
    } else res.writeHead(404).end();
  });
  await new Promise<void>(r => server.listen(port, "127.0.0.1", () => r()));
  return { base: `http://127.0.0.1:${port}`, server };
}

type Site = { d: Daemon; origin: string; gh: { base: string; server: Server }; stop(): Promise<void> };

/** A hosted daemon with GitHub sign-in (`fakeGitHub`), the bundled Terms
 * and Privacy Policy (`legal`), the public gallery and the landing page;
 * `home` starts it on a home kept from an earlier one. */
async function site(o: { legal?: boolean; home?: string; gh?: { base: string; server: Server } } = {}): Promise<Site> {
  const gh = o.gh ?? await fakeGitHub();
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\npublic_gallery = true\n`
    + `github_client_id = "e2e"\ngithub_client_secret_env = "CLAX_E2E_GH_SECRET"\ngithub_oauth_base = "${gh.base}"\ngithub_api_base = "${gh.base}"\n`
    + (o.legal === false ? "" : 'legal = "bundled"\n');
  const d = await startDaemon({ config, port, home: o.home, env: { CLAX_E2E_SETUP_SECRET: SECRET, CLAX_E2E_GH_SECRET: "e2e", CLAX_GALLERY__QUICKSTART: "true" } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}: its public origin would not match`);
  return { d, origin, gh, stop: async () => { await d.stop({ keepHome: !!o.home }); if (!o.gh) gh.server.close(); } };
}

/** A context of `browser` on `origin` with nothing in it but what `use` gives (the viewport, the theme). */
async function context(browser: Browser, use: object): Promise<BrowserContext> {
  return browser.newContext(use);
}

/** Calls `path` from a page of `ctx` on `origin` (the browser's own cookies, same-origin). */
async function call<T>(ctx: BrowserContext, origin: string, path: string, method = "GET", body?: unknown): Promise<{ status: number; json: T }> {
  const p = await ctx.newPage();
  await p.goto(`${origin}/healthz`);
  const r = await p.evaluate(async ([to, how, sent]) => {
    const res = await fetch(to, { method: how, headers: { "content-type": "application/json" }, body: how === "GET" ? undefined : JSON.stringify(sent ?? {}) });
    return { status: res.status, json: await res.json().catch(() => null) };
  }, [path, method, body] as const);
  await p.close();
  return r as { status: number; json: T };
}

/** The owner, Ada, set up with the secret in a context of her own. */
async function owner(browser: Browser, s: Site, use: object): Promise<BrowserContext> {
  const ctx = await context(browser, use);
  const r = await call(ctx, s.origin, "/api/hosted/setup", "POST", { secret: SECRET, name: "Ada" });
  expect(r.status, "setup").toBe(200);
  return ctx;
}

/** One page's load, as a staged load records it. */
type Stage = { stage: string | null; mounted: boolean; prompt: boolean; places: Record<string, Place | null>; bg: string };
type Visit = { order: string[]; stages: Stage[]; documents: string[]; first: Record<string, boolean> };

/** What the first frame shows: the header, the card with its heading, busy (or the landing's link and stage). */
const FIRST = `(() => {
  const seen = s => [...document.querySelectorAll(s)].some(e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== "hidden"; });
  const landing = location.pathname === "/start/";
  return {
    header: seen(${JSON.stringify(BAR())}) && seen(${JSON.stringify(BAR(" .home .mark"))}) && seen(${JSON.stringify(BAR(" .theme-switch"))}),
    body: landing ? seen(${JSON.stringify(BAR(" .primary"))}) && seen("#app .lframe") : seen("${CARD}") && seen("${CARD} h2") && !!document.querySelector("${CARD} h2")?.textContent?.trim(),
    // The landing's stage is the quickstart's own frame, loading as a document does.
    busy: landing || !!document.querySelector("#app main.signin[aria-busy=true]"),
  };
})()`;

/** Installs the recorders on `page`, and a log of the main frame's documents (each redirect with its status). */
async function record(page: Page): Promise<string[]> {
  // The gallery's event stream is the page's own, as where shared workers
  // are missing (shell/src/stream.ts): a shared worker's requests reach no
  // event of the page's, and would never finish at a stage.
  await page.addInitScript(() => { Reflect.deleteProperty(window, "SharedWorker"); });
  await recordLayoutShifts(page);
  await recordTexts(page, TEXTS);
  const documents: string[] = [];
  const path = (u: string) => { const x = new URL(u); return x.origin === new URL(page.url() === "about:blank" ? u : page.url()).origin || x.hostname === "localhost" ? `${x.pathname}${x.search}` : `${x.origin}${x.pathname}`; };
  page.on("response", res => {
    const req = res.request();
    if (!req.isNavigationRequest() || req.frame() !== page.mainFrame()) return;
    const loc = res.headers().location;
    documents.push(loc ? `${res.status()} ${path(res.url())} -> ${loc}` : `${res.status()} ${path(res.url())}`);
  });
  return documents;
}

/** Drives the load already started in `page` until `done` holds; answers what it saw. */
async function drive(page: Page, load: StagedLoad, done: string, documents: string[]): Promise<Visit> {
  const stages: Stage[] = [];
  let first: Record<string, boolean> = {};
  let missing = "";
  let framed = false;
  const each = async (stage: string | null) => {
    const places = await landmarks(page, SIGNIN_MARKS);
    if (!framed) { framed = true; await atFirst(); }
    const [mounted, prompt, bg] = await quietEval<[boolean, boolean, string]>(page, `[(() => { const f = window.claxFrame; const b = document.querySelector(${JSON.stringify(BAR())}); return !!f && !!b && !f.some(n => n === b || n.contains(b)); })(), !!document.querySelector("[data-testid=legal-prompt]"), getComputedStyle(document.body).backgroundColor]`);
    stages.push({ stage, mounted, prompt, places, bg });
  };
  const atFirst = async () => {
    first = await quietEval<Record<string, boolean>>(page, FIRST);
    await quietEval(page, `(() => { window.claxFrame = [...document.querySelectorAll("#app > *")]; })()`);
  };
  const order = await load.drive(async () => (missing = await quietEval<string>(page, done)) === "", { each }).catch((e: unknown) => {
    throw new Error(`${String(e)}\nstill missing: ${missing}\nreleased in order:\n  ${load.released.join("\n  ")}`);
  });
  await each("settled");
  return { order, stages, documents: [...documents], first };
}

/** Loads `url` in `page` staged until `done` (an expression answering what is missing, "" once settled). */
async function visit(page: Page, url: string, done: string): Promise<Visit> {
  const documents = await record(page);
  const load = await stageLoad(page, ORDER);
  await page.goto(url, { waitUntil: "commit" });
  return drive(page, load, done, documents);
}

/** The landmarks at every stage against the settled page's (the frame's included). */
function holds(stages: Stage[]): string[] {
  const end = stages.at(-1)!;
  const out: string[] = [];
  for (const s of stages.slice(0, -1)) for (const d of placesDiffer(SIGNIN_MARKS, s.places, end.places)) out.push(`after ${s.stage ?? "(the first frame)"}: ${d}`);
  return out;
}

/** Everything a visit is held to: what the first frame shows, its theme, nothing moving, each text written once, the landmarks. */
async function judge(page: Page, v: Visit, what: string): Promise<void> {
  const released = `${what}: released in order:\n  ${v.order.join("\n  ")}\ndocuments: ${v.documents.join(", ")}`;
  expect.soft(v.first, `${what}: the first frame, before any script ran`).toEqual({ header: true, body: true, busy: true });
  expect.soft(v.stages.map(s => s.bg).filter((b, i, a) => a.indexOf(b) === i), `${what}: the page's background at every stage`).toHaveLength(1);
  const shifts = await layoutShifts(page);
  expect.soft(shiftTotal(shifts), `${what}: ${formatShifts(shifts)}\n${released}`).toBe(0);
  const texts = await textHistory(page);
  for (const sel of TEXTS) expect.soft(shownTexts(texts[sel]).length, `${what}: ${formatTexts(sel, texts[sel])}`).toBeLessThanOrEqual(1);
  expect.soft(holds(v.stages), released).toEqual([]);
  expect.soft(await overlaps(page, SIGNIN_APART), `${what}: boxes that overlap, or leave the header, at the settled page`).toEqual([]);
}

/** What a sign-in page still misses once it is settled, by what it must show. */
const settled = (must: string) => `(() => {
  const m = [];
  if (!(${must})) m.push(${JSON.stringify(must)});
  if (document.querySelector("#app main[aria-busy=true]")) m.push("main no longer busy");
  return m.join(", ");
})()`;
const text = (s: string) => `document.body.innerText.includes(${JSON.stringify(s)})`;
const LANDING_DONE = `(() => {
  const f = document.querySelector("#app .lframe");
  const h = f?.contentDocument?.querySelector("h1");
  return h && f.contentDocument.readyState === "complete" ? "" : "the quickstart in the landing's frame";
})()`;

const FORMS = [
  ["desktop", { viewport: { width: 1280, height: 900 } }],
  ["phone", { viewport: { width: 390, height: 844 } }],
  ["phone with touch", { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true }],
] as const;

for (const [form, size] of FORMS) {
  for (const colorScheme of ["light", "dark"] as const) {
    const use = { ...size, colorScheme };
    test.describe(`on a ${form}, ${colorScheme}`, () => {
      test.use(use);

      test("each sign-in page's first frame is its frame, and nothing on it moves or is written twice", async ({ browser }) => {
        // Nine pages, each loaded one response at a time.
        test.setTimeout(HANG_MS);
        const s = await site();
        try {
          const ada = await owner(browser, s, use);
          // A page listed in the public gallery, which a signed-out /signin lists.
          const sid = await registerSession(s.d.base, s.d.token, "claude", "smooth");
          const aid = (await publishAs(s.d.base, s.d.token, sid.id, "Public roadmap", { "index.html": "<h1>Roadmap</h1>" })).artifact.id;
          expect((await call(ada, s.origin, `/api/hosted/artifacts/${aid}/link`, "PUT", { access: "viewer", listed: true })).status).toBe(200);
          const link = (await call<{ url: string }>(ada, s.origin, "/api/hosted/signin-links", "POST")).json.url;
          const invite = (await call<{ invite: { url: string } }>(ada, s.origin, "/api/people/invites", "POST", { name: "Mia" })).json.invite.url;
          const device = (await (await fetch(`${s.d.base}/api/hosted/device/code`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Pixel" }) })).json()) as { user_code: string };

          const pages: [string, () => Promise<BrowserContext>, string, string][] = [
            ["the landing page, from /", () => context(browser, use), "/", LANDING_DONE],
            ["/signin, signed out", () => context(browser, use), "/signin", settled(`!!document.querySelector("[data-testid=github-signin]") && !!document.querySelector("${CARD}.pub li") && !!document.querySelector("[data-testid=legal-notice]")`)],
            ["/signin, signed in", async () => ada, "/signin", settled(text("Sign out"))],
            ["/signin/<code>", () => context(browser, use), new URL(link).pathname, settled(text("Sign in as Ada"))],
            ["/signin/<code>, a spent code", () => context(browser, use), "/signin/nope", settled(text("This sign-in link is not valid"))],
            ["/join/<code>", () => context(browser, use), new URL(invite).pathname, settled(text("Join as Mia"))],
            ["/join/<code>, a spent code", () => context(browser, use), "/join/nope", settled(text("This invite is not valid"))],
            ["/device?code=, signed in", async () => ada, `/device?code=${device.user_code}`, settled(text("Approve"))],
            ["/device, signed out", () => context(browser, use), "/device", settled(`!!document.querySelector("[data-testid=github-signin]")`)],
          ];
          for (const [what, ctx, path, done] of pages) {
            const page = await (await ctx()).newPage();
            const v = await visit(page, `${s.origin}${path}`, done);
            await judge(page, v, what);
            if (what === "the landing page, from /") expect.soft(v.documents, what).toEqual(["303 / -> /start/", "200 /start/"]);
            else expect.soft(v.documents.length, `${what}: ${v.documents.join(", ")}`).toBe(1);
            await page.close();
          }
        } finally {
          await s.stop();
        }
      });

      test("setup's first frame is its frame, and nothing on it moves or is written twice", async ({ browser }) => {
        const s = await site();
        try {
          const page = await (await context(browser, use)).newPage();
          const v = await visit(page, `${s.origin}/setup?secret=${encodeURIComponent(SECRET)}`, settled(text("Create the first person")));
          await judge(page, v, "/setup");
        } finally {
          await s.stop();
        }
      });

      test("a signed-in browser sent to /signin?next= goes there by the server's redirect", async ({ browser }) => {
        const s = await site();
        try {
          const ada = await owner(browser, s, use);
          const page = await ada.newPage();
          const documents = await record(page);
          await page.goto(`${s.origin}/signin?next=${encodeURIComponent("/tags")}`);
          await page.waitForURL(`${s.origin}/tags`);
          expect(documents).toEqual(["303 /signin?next=%2Ftags -> /tags", "200 /tags"]);
        } finally {
          await s.stop();
        }
      });

      test("the journey: a signed-out visit to /, the landing, sign-in by GitHub, the gallery, sign-out", async ({ browser }) => {
        // Five pages, each loaded one response at a time.
        test.setTimeout(HANG_MS);
        const s = await site();
        try {
          // The owner is set up; the visitor is new, and signs up by GitHub.
          await owner(browser, s, use);
          const page = await (await context(browser, use)).newPage();
          const documents = await record(page);
          const load = await stageLoad(page, ORDER);
          await page.goto(`${s.origin}/`, { waitUntil: "commit" });
          const landing = await drive(page, load, LANDING_DONE, documents);
          await judge(page, landing, "the landing page");

          documents.length = 0;
          await page.locator("#app .gbar .primary").click();
          await page.waitForURL(`${s.origin}/signin`, { waitUntil: "commit" });
          const signin = await drive(page, load, settled(`!!document.querySelector("[data-testid=github-signin]")`), documents);
          await judge(page, signin, "/signin from the landing page");
          expect.soft(signin.documents, "Sign in on the landing page").toEqual(["200 /signin"]);

          documents.length = 0;
          await page.getByTestId("github-signin").click();
          await page.waitForURL(`${s.origin}/`, { waitUntil: "commit" });
          const gallery = await drive(page, load, `document.querySelector("#app main.gal a.card") && !document.querySelector("#app main.gal .gskel") ? "" : "the gallery's quickstart card"`, documents);
          // The callback answers a page that goes on to the gallery itself, a
          // same-origin navigation, so the gallery's load carries its block.
          expect.soft(gallery.documents.map(d => d.replace(/\?.*? ->/, " ->").replace(/(-> https?:\/\/[^/]+\/[^?]*)\?.*/, "$1").replace(/^(\d+ [^ ?]+)\?\S*$/, "$1")), "GitHub's sign-in, back to the gallery").toEqual([
            `303 /auth/github -> ${s.gh.base}/login/oauth/authorize`,
            `302 ${s.gh.base}/login/oauth/authorize -> ${s.origin}/auth/github/callback`,
            "200 /auth/github/callback",
            "200 /",
          ]);
          expect.soft(await page.evaluate(() => !!document.getElementById("clax-boot")), "the gallery's bootstrap block after GitHub").toBe(true);
          expect.soft(shiftTotal(await layoutShifts(page)), formatShifts(await layoutShifts(page))).toBe(0);
          expect.soft(await overlaps(page, [...SIGNIN_APART, ...GALLERY_APART]), "boxes that overlap at the settled page").toEqual([]);

          documents.length = 0;
          await page.goto(`${s.origin}/settings/account`, { waitUntil: "commit" });
          await drive(page, load, `[...document.querySelectorAll("button")].some(b => b.textContent === "Sign out") ? "" : "Sign out"`, documents);
          // Signing out loads its chunks unheld; the page it lands on is staged again.
          await page.unroute("**/*");
          documents.length = 0;
          await page.getByRole("button", { name: "Sign out" }).first().click();
          await page.waitForURL(`${s.origin}/signin`);
          expect.soft(documents, "signing out").toEqual(["200 /signin"]);
          // Settled, so none of its requests reaches the staged load of it again.
          await expect(page.getByTestId("github-signin")).toBeVisible();
          await expect(page.locator(".silegal a")).not.toHaveCount(0);
          documents.length = 0;
          const again = await stageLoad(page, ORDER);
          await page.reload({ waitUntil: "commit" });
          const out = await drive(page, again, settled(`!!document.querySelector("[data-testid=github-signin]")`), documents);
          await judge(page, out, "/signin after signing out");
        } finally {
          await s.stop();
        }
      });

      test("the Terms prompt is decided at the gallery's first render, not after it", async ({ browser }) => {
        // Ada was set up before the instance showed its Terms: she has not agreed to them.
        const gh = await fakeGitHub();
        const home = (await import("node:fs")).mkdtempSync(`${(await import("node:os")).tmpdir()}/clax-e2e-legal-`);
        let s = await site({ legal: false, home, gh });
        // Her browser's session cookie is the host's, whatever the port the daemon is on next.
        const ada = await owner(browser, s, use);
        await s.stop();
        s = await site({ legal: true, home, gh });
        try {
          const page = await ada.newPage();
          const v = await visit(page, `${s.origin}/`, `document.querySelector("[data-testid=legal-prompt]") && !document.querySelector("#app main.gal .gskel") ? "" : "the prompt over the gallery"`);
          const mounted = v.stages.filter(x => x.mounted);
          expect.soft(mounted.length, "stages with the gallery mounted").toBeGreaterThan(0);
          expect.soft(mounted.filter(x => !x.prompt).map(x => x.stage), "stages with the gallery mounted and no prompt over it").toEqual([]);
          expect.soft(shiftTotal(await layoutShifts(page))).toBe(0);
          expect.soft(await overlaps(page, [...SIGNIN_APART, ...GALLERY_APART]), "boxes that overlap at the settled page").toEqual([]);
        } finally {
          await s.stop();
          gh.server.close();
          (await import("node:fs")).rmSync(home, { recursive: true, force: true });
        }
      });
    });
  }
}
