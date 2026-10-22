import { readFileSync } from "node:fs";
import { test, expect, type Daemon, openArtifact, publishWith, nameField } from "./fixtures";
import { guestOn, hostedDaemon, openAt, ownerOf, shareByLink } from "./hosted-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const html = (name: string) => readFileSync(new URL(`./pages/${name}`, import.meta.url), "utf8");
const facts = async (f: import("@playwright/test").Frame) => {
  // Read until it holds facts: a reloading document starts over at "waiting".
  let out: Record<string, unknown> | null = null;
  await expect.poll(async () => {
    const t = await f.locator("#facts").textContent().catch(() => null);
    try { out = t === null || t === "waiting" ? null : JSON.parse(t); } catch { out = null; }
    return out !== null;
  }).toBe(true);
  return out!;
};

test(`user in the owner shell`, async ({ page, freshDaemon }) => {
  // A daemon of its own: the owner is one viewer per daemon, and this test
  // starts with it unnamed.
  d = freshDaemon;
  const { artifact } = await publishWith(d.base, d.token, `Who`, html("who.html"), { user: { scopes: ["profile"] }, db: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  expect(await facts(f)).toMatchObject({ isOwner: true, canEdit: true, dataWrite: true, filesWrite: true, idShape: true, name: "", isMe: true, stranger: "", other: null, search: 0 });
  const name = await nameField(page);
  await name.fill("Alex");
  await Promise.all([page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"), name.press("Enter")]);
  // The page reads who the viewer is from its site once: the next document knows the name.
  const next = await openArtifact(page, d.base, artifact.id, 1);
  await next.locator("#refresh").click();
  await expect(next.locator("#greeting")).toHaveText("Hello, Alex");
  await expect.poll(async () => (await facts(next)).search).toBe(1);
  expect(await facts(next)).toMatchObject({ name: "Alex", meResolved: "Alex", search: 1 });
});

test(`assets upload, display, list, and delete`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Gallery`, html("gallery.html"), { assets: {}, db: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1);
  await expect(f.locator("#status")).toHaveText("ready");
  await f.locator("#upload").click();
  await expect(f.locator("#status")).toHaveText(JSON.stringify({ loaded: 40, files: 1, type: "image/png" }));
  await f.locator("#remove").click();
  await expect(f.locator("#status")).toHaveText(JSON.stringify({ first: true, second: false }));
});

// A viewer who is not the owner: a hosted daemon's guest on a page shared by
// link (a local daemon frames pages for the owner alone).
test("a guest: user is not the owner, resolves other viewers by name, and names them from the next document once named; data and assets are read-only", async ({ browser }) => {
  const { d: h, origin } = await hostedDaemon();
  try {
    const owner = await ownerOf(browser, origin, "Ärger Ölund");
    const other = await owner.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).viewer.public_id as string);
    const who = html("who.html").replace('data-other="u_ffffffffffffffffffffff"', `data-other="${other}"`);
    const { artifact } = await publishWith(h.base, h.token, "Who guest", who, { user: { scopes: ["profile"] }, db: {} });
    const { artifact: g } = await publishWith(h.base, h.token, "Gallery guest", html("gallery.html"), { assets: {} });
    await shareByLink(owner, artifact.id);
    await shareByLink(owner, g.id);
    const { page, frame: first } = await guestOn(browser, origin, artifact.id);
    // A guest has no viewer of their own until named, and named writes
    // only where a db rule names `public` (docs/contract.md, the site's level).
    expect(await facts(first)).toMatchObject({ isOwner: false, canEdit: false, dataWrite: false, filesWrite: false, idShape: false });
    const name = await nameField(page);
    await name.fill("Guest Reader");
    const [named] = await Promise.all([page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"), name.press("Enter")]);
    expect(named.status(), await named.text()).toBe(200);
    const f = await openAt(page, origin, artifact.id, 1);
    expect(await facts(f)).toMatchObject({ name: "Guest Reader", isOwner: false, canEdit: false, dataWrite: false, filesWrite: false, idShape: true, search: 0, other: "Ärger Ölund", stranger: "" });
    const f2 = await openAt(await page.context().newPage(), origin, g.id, 1);
    await expect(f2.locator("#status")).toHaveText("read-only");
  } finally {
    await h.stop();
  }
});
