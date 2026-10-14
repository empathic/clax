import { test, expect, type Daemon, openArtifact, postThread, publish, setName, startDaemon } from "./fixtures";

// Live seats (spec 2026-10-11-clax-dev-hosted-design §16.1, §16.2): a daemon
// with one people seat, which guests may take. The first named viewer holds
// it; the second is a spectator, read-only with the banner, kept current by
// the snapshot, and offered Join live once the first leaves.
let d: Daemon;
test.beforeAll(async () => { d = await startDaemon({ env: { CLAX_LIVE_PEOPLE_SEATS: "1", CLAX_LIVE_GUEST_SEATS: "1" } }); });
test.afterAll(async () => { await d.stop(); });

const BANNER = "Lots of people are here, so you're viewing: comments and live activity are paused.";

test("a viewer beyond the live seats is a read-only spectator until a seat frees and they join live", async ({ browser }) => {
  const { artifact } = await publish(d.base, d.token, "Seats", { "index.html": "<main><h2>Quarterly goals</h2></main>" });
  const t = await postThread(d.base, artifact.id, "Two columns");
  const ann = await (await browser.newContext()).newPage();
  const bob = await (await browser.newContext()).newPage();
  await openArtifact(ann, d.base, artifact.id, 1, "sandbox", { lan: true });
  await setName(ann, "Ann");
  await expect(ann.getByRole("button", { name: /^Comment/ })).toBeEnabled();
  // Ann's renamed stream holds the one seat before Bob arrives (a snapshot
  // is shared for 5 s, so a newer one says so).
  await expect.poll(async () => (await (await fetch(`${d.base}/api/artifacts/${artifact.id}/snapshot`)).json()).seats_free, { timeout: 20_000 }).toBe(0);
  // Bob's live layers sockets: a spectator holds none open (spec §16.1).
  const sockets: { closed: boolean }[] = [];
  bob.on("websocket", ws => {
    if (!ws.url().endsWith(`/api/artifacts/${artifact.id}/live`)) return;
    const s = { closed: false };
    sockets.push(s);
    ws.on("close", () => { s.closed = true; });
  });
  const liveOpen = () => sockets.filter(s => !s.closed).length;
  await openArtifact(bob, d.base, artifact.id, 1, "sandbox", { lan: true });
  await setName(bob, "Bob");
  const banner = bob.locator(".banner.spectate");
  await expect(banner).toContainText(BANNER);
  await expect.poll(liveOpen).toBe(0);
  await expect(banner.getByRole("button", { name: "Join live" })).toHaveCount(0);
  // Spectators have no chip: the top bar counts everyone viewing instead.
  await expect(bob.locator(".who .viewing")).toHaveText("2 viewing", { timeout: 20_000 });
  await expect(bob.locator(".who .tok.p")).toHaveText(["AN"]);
  await expect(bob.getByRole("button", { name: /^Comment/ })).toBeDisabled();
  if (!(await bob.locator("aside.sidebar").isVisible())) await bob.getByRole("button", { name: /Threads/ }).first().click();
  const card = bob.locator(`.thread-card[data-thread="${t.id}"]`);
  await expect(card).toBeVisible();
  await expect(card.getByRole("button", { name: "Resolve" })).toHaveCount(0);
  await expect(bob.getByRole("textbox", { name: "Reply" })).toHaveCount(0);
  // The snapshot keeps the spectator current: a thread posted now arrives.
  const t2 = await postThread(d.base, artifact.id, "A later thread");
  await expect(bob.locator(`.thread-card[data-thread="${t2.id}"]`)).toBeVisible({ timeout: 20_000 });
  // Ann leaves; her seat frees, and Bob is offered it.
  await ann.context().close();
  const join = banner.getByRole("button", { name: "Join live" });
  // Within a poll of a snapshot built after she left.
  await expect(join).toBeVisible({ timeout: 20_000 });
  await join.click();
  await expect(banner).toHaveCount(0);
  // Seated again, the view opens its live socket.
  await expect.poll(liveOpen, { timeout: 20_000 }).toBe(1);
  await expect(bob.getByRole("button", { name: /^Comment/ })).toBeEnabled();
  await expect(card.getByRole("button", { name: "Resolve" })).toBeVisible();
  await bob.context().close();
});
