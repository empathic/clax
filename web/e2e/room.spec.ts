import { readFileSync } from "node:fs";
import { test, expect, type Daemon, openArtifact, publishWith, setName } from "./fixtures";
import { guestOn, hostedDaemon, ownerOf, shareByLink } from "./hosted-helpers";
import { settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = readFileSync(new URL("./pages/room.html", import.meta.url), "utf8");
const CAPS = { room: { topics: { reaction: "interact" } } };

test(`two tabs see each other's presence, events, and named rooms`, async ({ context }) => {
  const { artifact } = await publishWith(d.base, d.token, `Room`, PAGE, CAPS);
  const a = await context.newPage();
  const b = await context.newPage();
  const fa = await openArtifact(a, d.base, artifact.id, 1);
  const fb = await openArtifact(b, d.base, artifact.id, 1);
  await expect(fa.locator("#status")).toHaveText("connected");
  await expect(fb.locator("#status")).toHaveText("connected");
  await expect(fa.locator("#count")).toHaveText("2");
  await expect(fb.locator("#count")).toHaveText("2");

  await fa.locator("#pick-b").click();
  await expect(fa.locator("#peers")).toContainText("this tab: B");
  await expect(fb.locator("#peers")).toContainText("your other tab: B");

  await fb.locator("#wave").click();
  await expect(fa.locator("#log")).toContainText("wave from your other tab");
  await expect(fb.locator("#log")).toContainText("wave from this tab");

  await fa.locator("#join").click();
  await expect(fa.locator("#table-count")).toHaveText("1");
  await fb.locator("#join").click();
  await expect(fa.locator("#table-count")).toHaveText("2");
  await expect(fb.locator("#table-count")).toHaveText("2");
  await fa.locator("#leave").click();
  await expect(fb.locator("#table-count")).toHaveText("1");

  await b.close();
  await expect(fa.locator("#count")).toHaveText("1");
});

test(`a reload is a new peer, and the old one leaves`, async ({ context }) => {
  const { artifact } = await publishWith(d.base, d.token, `Room reload`, PAGE, CAPS);
  const a = await context.newPage();
  const b = await context.newPage();
  const fa = await openArtifact(a, d.base, artifact.id, 1);
  await openArtifact(b, d.base, artifact.id, 1);
  await expect(fa.locator("#count")).toHaveText("2");
  const before = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
  await b.reload();
  await expect.poll(async () => {
    const now = await fa.locator("#peers li").evaluateAll(els => els.map(e => (e as HTMLElement).dataset.peer));
    return now.length === 2 && now.some(p => !before.includes(p));
  }, { timeout: 15_000 }).toBe(true);
});

/** A foreign document that greets nobody and keeps every message it receives. */
const RECORDER = "<!doctype html><title>Elsewhere</title><script>window.got = []; addEventListener(\"message\", e => window.got.push(e.data));</script><p>elsewhere</p>";

test("a document that leaves takes its peer with it, and nothing of the room reaches the next one", async ({ context }) => {
  const { artifact } = await publishWith(d.base, d.token, "Room leave", PAGE, CAPS);
  const a = await context.newPage();
  const b = await context.newPage();
  await a.route("http://foreign.test/**", r => r.fulfill({ contentType: "text/html", body: RECORDER }));
  const fa = await openArtifact(a, d.base, artifact.id, 1);
  const fb = await openArtifact(b, d.base, artifact.id, 1);
  await expect(fa.locator("#count")).toHaveText("2");
  await expect(fb.locator("#count")).toHaveText("2");
  // The page sends its own frame elsewhere; B stays quiet until A's peer has left.
  await fa.evaluate(() => { location.href = "http://foreign.test/elsewhere"; });
  await expect(fb.locator("#count")).toHaveText("1");
  await fb.locator("#pick-b").click();
  await fb.locator("#wave").click();
  await expect(fb.locator("#log")).toContainText("wave from this tab");
  await expect.poll(() => a.frame({ url: /foreign\.test/ }) !== null).toBe(true);
  const rec = a.frame({ url: /foreign\.test/ })!;
  // Long enough for B's presence and wave to have reached A's shell, were its socket still open.
  await settle(a);
  const got = await rec.evaluate(() => (window as unknown as { got: { type?: string }[] }).got);
  expect(got.filter(m => m && typeof m === "object" && String(m.type).startsWith("clax:"))).toEqual([]);
});

// A viewer who is not the owner: a hosted daemon's guest, on a page the
// owner shares by link (a local daemon frames pages for the owner alone).
// The site's socket is at the level its session's holder has
// (clax_http::db_caller::site_caller): a link viewer's or a guest's is
// public, named or not, so an interact topic stays closed to them; naming
// themselves reconnects the room under the same peer.
test("guest: the room is at a guest's level, named or not, and naming keeps its peer", async ({ browser }) => {
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await publishWith(h.base, h.token, "Room guest", PAGE, CAPS);
    await shareByLink(await ownerOf(browser, origin), artifact.id);
    const { page, frame: f } = await guestOn(browser, origin, artifact.id);
    await expect(f.locator("#status")).toHaveText("connected");
    await expect(f.locator("#count")).toHaveText("1");
    const peer = await f.locator("#peers li").getAttribute("data-peer");
    await f.locator("#pick-a").click();
    await expect(f.locator("#peers")).toContainText("this tab: A");
    await f.locator("#wave").click();
    await expect(f.locator("#log li")).toHaveText(["refused: not_permitted"]);
    await setName(page, "Ben");
    await expect(f.locator("#status")).toHaveText("connected");
    await f.locator("#wave").click();
    await expect(f.locator("#log li")).toHaveText(["refused: not_permitted", "refused: not_permitted"]);
    await expect(f.locator("#count")).toHaveText("1");
    await expect(f.locator("#peers li")).toHaveAttribute("data-peer", peer!);
  } finally {
    await h.stop();
  }
});
