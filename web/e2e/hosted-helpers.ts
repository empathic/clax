// Several viewers of one artifact in browser tests. A local daemon serves
// this machine alone (docs/usage.md), so every browser of it is the owner:
// a second viewer is a hosted daemon's guest, on a page the owner shares by
// link (crates/clax-server/src/hosted.rs), as in hosted.spec.ts.
import { createServer } from "node:net";
import type { Browser, Frame, Page } from "@playwright/test";
import { type Daemon, NO_KEY_CONFIG, contentFrame, expect, startDaemon, test } from "./fixtures";

/** The setup secret the hosted daemons here are started with. */
export const SECRET = "e2e setup secret";

/** A port nothing listens on now. */
export function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

/** A hosted daemon on `http://localhost:<port>`, with `extra` in its
 * `[hosted]` table, `env` in its environment and `base` (the `[sample]`
 * settings, `NO_KEY_CONFIG` by default) before it; the test skips when the
 * daemon took another port than its public origin names. */
export async function hostedDaemon(extra = "", env: Record<string, string> = {}, base = NO_KEY_CONFIG): Promise<{ d: Daemon; origin: string }> {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${base}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n${extra}`;
  const d = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET, ...env } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}: its public origin would not match`);
  return { d, origin };
}

/** The owner's browser, set up as `name` with the secret. */
export async function ownerOf(browser: Browser, origin: string, name = "Ada"): Promise<Page> {
  const owner = await (await browser.newContext()).newPage();
  await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
  await owner.getByLabel("Your name").fill(name);
  await owner.getByRole("button", { name: "Create the first person" }).click();
  await owner.waitForURL(`${origin}/`);
  return owner;
}

/** Shares artifact `aid` by link from the owner's browser, at `access`. */
export async function shareByLink(owner: Page, aid: string, access = "commenter"): Promise<void> {
  const status = await owner.evaluate(async ([id, a]) => {
    const r = await fetch(`/api/hosted/artifacts/${id}/link`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ access: a }) });
    return r.status;
  }, [aid, access] as const);
  expect(status).toBe(200);
}

/** Opens `aid` in `page` at `origin` and waits for version `n` in its frame. */
export async function openAt(page: Page, origin: string, aid: string, n: number): Promise<Frame> {
  await page.goto(`${origin}/a/${aid}`);
  return contentFrame(page, aid, n);
}

/** A guest's browser (a new context, nothing but the link) on `aid`, with
 * version `n` in its frame. */
export async function guestOn(browser: Browser, origin: string, aid: string, n = 1): Promise<{ page: Page; frame: Frame }> {
  const page = await (await browser.newContext()).newPage();
  return { page, frame: await openAt(page, origin, aid, n) };
}

/** A thread `page`'s browser posts on `aid` v1, anchored to `selector`, as the shell posts it. */
export async function threadIn(page: Page, aid: string, body: string, selector = "body > main > h2"): Promise<string> {
  return page.evaluate(async ([id, text, sel]) => {
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: sel, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", text);
    f.set("version", "1");
    const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: f });
    if (!r.ok) throw new Error(`thread: ${r.status} ${await r.text()}`);
    return (await r.json()).thread.id as string;
  }, [aid, body, selector] as const);
}

/** What a person may do on artifact `aid`: read it, comment on it (reply
 * and resolve), and read and write its page data at `level`. */
export const rightsOn = (aid: string, level = "interact") => [
  { type: "clax-artifacts", actions: ["read"], locations: [aid] },
  { type: "clax-comments", actions: ["read", "write", "reply", "resolve"], locations: [aid] },
  { type: "clax-db", actions: ["read", "write"], level, locations: [aid] },
];

/** A person `name` the owner invites with `rights`, joined in a browser of
 * their own (a new context), at the gallery. */
export async function memberOf(browser: Browser, owner: Page, origin: string, name: string, rights: unknown[]): Promise<Page> {
  const url = await owner.evaluate(async ([n, r]) => {
    const res = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: n, rights: r }) });
    if (!res.ok) throw new Error(`invite: ${res.status} ${await res.text()}`);
    return (await res.json()).invite.url as string;
  }, [name, rights] as const);
  const page = await (await browser.newContext()).newPage();
  await page.goto(url);
  await page.getByRole("button", { name: `Join as ${name}` }).click();
  await page.waitForURL(`${origin}/`);
  return page;
}
