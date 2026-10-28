import { type Frame, type Locator, type Page } from "@playwright/test";
import { test, expect, type Daemon, approve, contentFrame, openArtifact, postThread, publishWith, reach, registerSession } from "./fixtures";
import { hostedDaemon, memberOf, ownerOf, rightsOn } from "./hosted-helpers";
import { advance, quietEval } from "./time";

// The shell capability against the real shell in Chromium: the ring is
// judged on the control's own box, on desktop and on a phone.

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

/** The ring's distance from the control's box (shell/src/ui/coach-mark.ts). */
const COACH_PAD = 4;
const NAMES = ["comment", "threads", "versions", "publish", "fork", "send"] as const;

/** A page with a button per call, #hl-<name>, #can-<name>, #on, #off and the key O (which acts as #off), each writing its result (or its error code) to #out. */
const TOUR = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Tour</title>
<style>:root { --bg: #fff; --fg: #1a1a1a; } @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #14161a; --fg: #ececec; } }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; } button { margin: 2px; }</style></head>
<body><main><h2>Tour</h2><div id="buttons"></div><p>out <code id="out">-</code></p><p>load <code id="load">-</code></p><p>probe <code id="probe">-</code></p></main>
<script>
(async () => {
  const out = document.getElementById("out");
  const say = v => { out.textContent = JSON.stringify(v); };
  const shell = await claude.use("shell");
  document.getElementById("probe").textContent = shell === null ? "null" : Object.keys(shell).sort().join(",");
  const call = fn => async () => { try { say(await fn()); } catch (e) { say({ code: e.code }); } };
  const add = (id, label, fn) => {
    const b = document.createElement("button");
    b.id = id; b.textContent = label; b.onclick = call(fn);
    document.getElementById("buttons").append(b);
  };
  if (!shell) return;
  for (const n of ${JSON.stringify(NAMES)}) {
    add("hl-" + n, "highlight " + n, () => shell.highlight(n));
    add("can-" + n, "can " + n, () => shell.can(n));
  }
  add("on", "on", () => shell.commentMode(true));
  add("off", "off", () => shell.commentMode(false));
  // In comment mode the bridge takes the page's clicks as picks, so the page also leaves it from a key.
  addEventListener("keydown", e => { if (e.key === "o") call(() => shell.commentMode(false))(); });
  try { document.getElementById("load").textContent = JSON.stringify({ load: await shell.commentMode(true) }); }
  catch (e) { document.getElementById("load").textContent = JSON.stringify({ load: e.code }); }
})();
</script></body></html>`;

async function tour(page: Page, viewport = { width: 1280, height: 800 }, base = d) {
  await page.setViewportSize(viewport);
  const { artifact } = await publishWith(base.base, base.token, `Tour`, TOUR, { shell: {} });
  await approve(base.base, base.token, artifact.id);
  const f = await openArtifact(page, base.base, artifact.id, 1);
  await expect(f.locator("#load")).not.toHaveText("-");
  return { f, aid: artifact.id };
}

/** Clicks the page's `#id` and returns what it wrote to #out. */
async function press(page: Page, f: Frame, id: string): Promise<string> {
  await f.locator("#out").evaluate(e => { e.textContent = "-"; });
  await reach(page, f.locator(`#${id}`));
  await page.mouse.down();
  await page.mouse.up();
  await expect(f.locator("#out")).not.toHaveText("-");
  return (await f.locator("#out").textContent())!;
}

const ring = (page: Page) => page.locator(".coach-mark");

/** Expects the ring to enclose `control`'s box, each edge within COACH_PAD + 1 px of it. */
async function expectRingAround(page: Page, control: Locator) {
  await expect(ring(page)).toHaveCount(1);
  await expect.poll(async () => {
    const r = await ring(page).boundingBox();
    const c = await control.boundingBox();
    if (!r || !c) return null;
    const gaps = [c.x - r.x, c.y - r.y, r.x + r.width - (c.x + c.width), r.y + r.height - (c.y + c.height)];
    return gaps.every(g => g >= -0.5 && g <= COACH_PAD + 1);
  }).toBe(true);
}

test(`a_highlight_rings_the_real_comment_button`, async ({ page }) => {
  const { f } = await tour(page);
  await expect(page.locator("button.comment")).toHaveAttribute("aria-pressed", "false");
  expect(await press(page, f, "hl-comment")).toBe("true");
  await expectRingAround(page, page.locator("button.comment"));
  const r = (await ring(page).boundingBox())!;
  const cx = r.x + r.width / 2;
  const cy = r.y + r.height / 2;
  expect(await quietEval<boolean>(page, `document.elementFromPoint(${cx}, ${cy}).closest('[data-shell-control="comment"]') !== null`)).toBe(true);
  await page.mouse.click(cx, cy);
  await expect(page.locator('button.comment[aria-pressed="true"]')).toHaveCount(1);
  await expect(ring(page)).toHaveCount(0);
});

test(`a_ring_goes_after_six_seconds`, async ({ page }) => {
  const { f } = await tour(page);
  expect(await press(page, f, "hl-threads")).toBe("true");
  await expectRingAround(page, page.locator("button.threads"));
  // One evaluate reads the ring's due time off the clock and steps to 1 ms
  // short of it, so real time between the two cannot carry it past.
  await quietEval(page, `globalThis.claxTestClock.advance(globalThis.claxTestClock.dueIn("coach-mark") - 1)`);
  await expect(ring(page)).toHaveCount(1);
  await advance(page, 1);
  await expect(ring(page)).toHaveCount(0);
});

test(`app_mode_shows_no_controls`, async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { artifact } = await publishWith(d.base, d.token, `Tour`, TOUR, { shell: {} });
  await approve(d.base, d.token, artifact.id);
  await page.goto(`${d.base}/a/${artifact.id}?app=1`);
  const f = await contentFrame(page, artifact.id, 1);
  await expect(f.locator("#load")).not.toHaveText("-");
  // The Clax app hides Clax's own bar: `shell` is served, no control reads
  // as shown and nothing is ringed.
  await expect(f.locator("#probe")).not.toHaveText("null");
  for (const name of NAMES) {
    expect(await press(page, f, `can-${name}`), name).toBe("false");
    expect(await press(page, f, `hl-${name}`), name).toBe("false");
  }
  await expect(ring(page)).toHaveCount(0);
});

test(`every_desktop_control_the_owner_sees_is_ringable`, async ({ page }) => {
  const { f } = await tour(page);
  for (const name of ["comment", "threads", "versions", "fork", "publish"]) {
    expect(await press(page, f, `can-${name}`), name).toBe("true");
    expect(await press(page, f, `hl-${name}`), name).toBe("true");
    const shown = page.locator(`[data-shell-control="${name}"]:visible`).first();
    await expectRingAround(page, shown);
    await advance(page, 6_000);
    await expect(ring(page)).toHaveCount(0);
  }
});

test(`on_a_phone_the_menu_controls_read_false_and_threads_is_the_tab`, async ({ page }) => {
  const { f } = await tour(page, { width: 390, height: 800 });
  for (const name of ["versions", "publish", "fork"]) {
    expect(await press(page, f, `can-${name}`), name).toBe("false");
    expect(await press(page, f, `hl-${name}`), name).toBe("false");
    await expect(ring(page)).toHaveCount(0);
  }
  expect(await press(page, f, "hl-threads")).toBe("true");
  await expectRingAround(page, page.locator('.phone-tabs [data-shell-control="threads"]'));
});

test(`narrowing_to_a_phone_removes_a_ring_on_fork`, async ({ page }) => {
  const { f } = await tour(page);
  expect(await press(page, f, "hl-fork")).toBe("true");
  await expectRingAround(page, page.locator('[data-shell-control="fork"]:visible'));
  await page.setViewportSize({ width: 390, height: 800 });
  await expect(ring(page)).toHaveCount(0);
});

test(`comment_mode_needs_a_click_in_the_page`, async ({ page }) => {
  const { f } = await tour(page);
  await expect(f.locator("#load")).toHaveText(JSON.stringify({ load: "no_gesture" }));
  await expect(page.locator("button.comment")).toHaveAttribute("aria-pressed", "false");
  expect(await press(page, f, "on")).toBe("true");
  await expect(page.locator("button.comment")).toHaveAttribute("aria-pressed", "true");
  // The click that turned it on left the page focused; its key press is the next gesture.
  await f.locator("#out").evaluate(e => { e.textContent = "-"; });
  await page.keyboard.press("o");
  await expect(f.locator("#out")).toHaveText("false");
  await expect(page.locator("button.comment")).toHaveAttribute("aria-pressed", "false");
});

test(`the_ring_follows_a_scrolling_container_and_goes_when_its_control_leaves_it`, async ({ page, freshDaemon }) => {
  const sid = (await registerSession(freshDaemon.base, freshDaemon.token, "claude", "shell-scroll")).id;
  const res = await fetch(`${freshDaemon.base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${freshDaemon.token}`, "x-clax-session": sid },
    body: JSON.stringify({ title: "Tour", capabilities: { shell: {} }, files: { "index.html": { content: TOUR.replace("<h2>Tour</h2>", "<h2>Quarterly goals</h2>"), encoding: "utf8" } } }),
  });
  const { artifact } = (await res.json()) as { artifact: { id: string } };
  await approve(freshDaemon.base, freshDaemon.token, artifact.id);
  for (let i = 0; i < 25; i++) await postThread(freshDaemon.base, artifact.id, `item ${i}`);
  await page.setViewportSize({ width: 1280, height: 800 });
  const f = await openArtifact(page, freshDaemon.base, artifact.id, 1);
  await expect(f.locator("#load")).not.toHaveText("-");
  if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
  const send = page.locator("aside.sidebar button.send-unsent");
  await expect(send).toBeVisible();
  const sidebar = page.locator("aside.sidebar");
  await sidebar.evaluate(e => { e.scrollTop = 0; });

  expect(await press(page, f, "hl-send")).toBe("true");
  await expectRingAround(page, send);
  const before = (await ring(page).boundingBox())!.y;
  await sidebar.evaluate(e => { e.scrollTop = 20; });
  await expectRingAround(page, send);
  expect((await ring(page).boundingBox())!.y).toBeLessThan(before);

  await sidebar.evaluate(e => { e.scrollTop = e.scrollHeight; });
  await expect(ring(page)).toHaveCount(0);

  // Pointing at it again brings it back into view, then rings it.
  expect(await press(page, f, "hl-send")).toBe("true");
  await expectRingAround(page, send);
});

test(`a_commenter_gets_no_shell_and_an_editor_does`, async ({ browser }) => {
  const { d: hd, origin } = await hostedDaemon();
  try {
    const owner = await ownerOf(browser, origin);
    const { artifact } = await publishWith(hd.base, hd.token, "Tour", TOUR, { shell: {} });
    const aid = artifact.id;
    await approve(hd.base, hd.token, aid);
    const sam = await memberOf(browser, owner, origin, "Sam", rightsOn(aid));
    await sam.goto(`${origin}/a/${aid}`);
    const sf = await contentFrame(sam, aid, 1);
    await expect(sf.locator("#probe")).toHaveText("null");

    const rights = rightsOn(aid).map(r => (r.type === "clax-artifacts" ? { ...r, actions: ["read", "update"] } : r));
    const eve = await memberOf(browser, owner, origin, "Eve", rights);
    await eve.goto(`${origin}/a/${aid}`);
    const ef = await contentFrame(eve, aid, 1);
    await expect(ef.locator("#probe")).toHaveText("address,can,commentMode,highlight");
  } finally {
    await hd.stop();
  }
});
