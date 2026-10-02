// Echo's chrome in a real browser: the favicon, the theme before first
// paint, and the shell's keys (C and ?), which belong to the page while it
// has focus and to a dialog while one is open, and are held while the viewer
// may still be typing for the page (spec §8, "Keys"; decisions Q2 and Q6).
import { expect, test, type Frame, type Page } from "@playwright/test";
import { api, contentFrame, openArtifact, postThread, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = `<!doctype html><html><head><title>Keys</title></head><body><main><h2 id="t">Target</h2><p>Type here.</p></main></body></html>`;

/** The body's background at the first moment the body exists, before the shell's script runs. */
async function firstBackground(page: Page, path: string): Promise<string> {
  await page.goto(`${d.base}${path}`);
  return page.evaluate(() => (window as unknown as { firstBg: string }).firstBg);
}

test("both entries name the Echo mark as their icon, and it is served", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Icon", PAGE, {});
  for (const path of ["/", `/a/${artifact.id}`]) {
    await page.goto(`${d.base}${path}`);
    await expect(page.locator('link[rel="icon"]')).toHaveAttribute("href", "/_clax/mark.svg");
  }
  const res = await page.request.get(`${d.base}/_clax/mark.svg`);
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toContain("image/svg+xml");
});

test("the first paint already has the theme: the system's, or the stored choice over it", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Theme", PAGE, {});
  await page.addInitScript(() => {
    new MutationObserver((_, o) => {
      if (!document.body) return;
      (window as unknown as { firstBg: string }).firstBg = getComputedStyle(document.body).backgroundColor;
      o.disconnect();
    }).observe(document, { childList: true, subtree: true });
  });
  const DARK = "rgb(26, 13, 9)", LIGHT = "rgb(251, 244, 241)";
  for (const [scheme, choice, want] of [["dark", null, DARK], ["light", "dark", DARK], ["dark", "light", LIGHT]] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto(`${d.base}/`);
    await page.evaluate(c => { if (c) localStorage.setItem("clax.theme", c); else localStorage.removeItem("clax.theme"); }, choice);
    for (const path of ["/", `/a/${artifact.id}`]) expect(await firstBackground(page, path), `${scheme} ${choice} ${path}`).toBe(want);
  }
});

test("keys pressed in the page are the page's: C and ? do nothing in the shell", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Frame keys", PAGE, {});
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  await page.locator("body").press("c");
  await expect(comment).toHaveAttribute("aria-pressed", "true");
  await page.locator("body").press("c");
  await expect(comment).toHaveAttribute("aria-pressed", "false");
  await frame.locator("p").click();
  await page.keyboard.press("c");
  await page.keyboard.press("Shift+?");
  await page.waitForTimeout(200);
  await expect(comment).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("C and ? are the shell's only letter keys: T, J, K, S, R and Enter do nothing", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Only keys", PAGE, { comments: {} });
  const t = await postThread(d.base, artifact.id, "Check this", "#t");
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const card = page.locator(`[data-thread="${t.id}"]`);
  await card.locator(".card-head").click();
  await expect(card).toHaveClass(/selected/);
  const threads = page.getByRole("button", { name: /^Threads/ });
  const panel = await threads.getAttribute("aria-pressed");
  await page.locator(".topbar h1").click();
  for (const k of ["t", "j", "k", "s", "r", "Shift+S", "Enter"]) await page.keyboard.press(k);
  await page.waitForTimeout(300);
  await expect(threads).toHaveAttribute("aria-pressed", panel!);
  await expect(card).toHaveClass(/selected/);
  await expect(card.getByRole("textbox", { name: "Reply" })).not.toBeFocused();
  const { threads: list } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { id: string; status: string; sent_to_agent: boolean }[] };
  expect(list.find(x => x.id === t.id)).toMatchObject({ status: "open", sent_to_agent: false });
  await page.keyboard.press("Shift+?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet.locator("dt")).toHaveText(["C", "?", "Esc"]);
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
});

// The page republishes itself on the viewer's first key in it, so the shell
// reloads while they are still typing for the page.
const REPUBLISH = `<!doctype html><html><head><title>Republish</title></head><body><main><h2 id="t">Target</h2><p>Type here.</p></main><script>
const next = "<!doctype html><html><head><title>Next</title></head><body><main><h2 id=t>Target</h2><p>Published.</p></main></body></html>";
claude.use("artifact").then(a => {
  let done = false;
  addEventListener("keydown", () => { if (!done) { done = true; a.publish(next).then(() => { document.title = "ok"; }, e => { document.title = e.code; }); } });
});
</script></body></html>`;

test("after a reload the page's publish caused, the viewer's typing stays inert until they press in the shell", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Republish keys", REPUBLISH, { artifact: {} });
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  await frame.locator("p").click();
  const reloaded = page.waitForEvent("load");
  await page.keyboard.type("x");
  await reloaded;
  await expect((await contentFrame(page, artifact.id, 2)).locator("p")).toHaveText("Published.");
  await page.keyboard.type("just c");
  await page.keyboard.press("Shift+?");
  await page.waitForTimeout(300);
  await expect(comment).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toHaveCount(0);
  // The viewer's own press in the shell gives the keys back, and a later load starts with them.
  await page.locator(".topbar h1").click();
  await page.keyboard.press("c");
  await expect(comment).toHaveAttribute("aria-pressed", "true");
  await page.reload();
  await expect(comment).toHaveAttribute("aria-pressed", "false");
  await page.locator("body").press("c");
  await expect(comment).toHaveAttribute("aria-pressed", "true");
});

// The attack: a page raises its consent prompt while the viewer types for it,
// so their next keys land on the prompt's button in the shell. None of them
// may open the sheet, turn comment mode on, send, resolve, or change the view.
const CONSENT = `<!doctype html><html><head><title>Consent</title></head><body><main><h2 id="t">Target</h2><p>Type here.</p></main><script>
claude.use("permissions").then(perm => {
  let asked = false;
  addEventListener("keydown", () => { if (!asked) { asked = true; perm.request(["comments"]); } });
});
</script></body></html>`;

test("a consent prompt raised while the viewer types takes the keys: C, ? and the rest do nothing", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Consent keys", CONSENT, { comments: {} });
  const t = await postThread(d.base, artifact.id, "Check this", "#t");
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const card = page.locator(`[data-thread="${t.id}"]`);
  await card.locator(".card-head").click();
  await expect(card).toHaveClass(/selected/);
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  const threads = page.getByRole("button", { name: /^Threads/ });
  const panel = await threads.getAttribute("aria-pressed");
  await frame.locator("p").click();
  await page.keyboard.type("h");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("post comments on this artifact under your name");
  await expect(dialog.getByRole("button", { name: "Don't allow" })).toBeFocused();
  for (const k of ["s", "r", "c", "t", "Shift+?", "j", "S", "R"]) await page.keyboard.press(k);
  await page.waitForTimeout(300);
  await expect(dialog).toHaveCount(1);
  await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toHaveCount(0);
  await expect(comment).toHaveAttribute("aria-pressed", "false");
  await expect(threads).toHaveAttribute("aria-pressed", panel!);
  const { threads: list } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { id: string; status: string; sent_to_agent: boolean }[] };
  expect(list.find(x => x.id === t.id)).toMatchObject({ status: "open", sent_to_agent: false });
  await dialog.getByRole("button", { name: "Don't allow" }).click();
  await expect(dialog).toHaveCount(0);
});

// The same attack one key later: the viewer's next key for the page closes
// the prompt (Space or Enter press its focused button, Escape dismisses it)
// and focus falls to the shell's body. The keys after it must still do
// nothing, until the viewer presses in the shell.
for (const closer of ["Space", "Enter", "Escape"]) {
  test(`after a page's prompt closes by ${closer}, the viewer's typing stays inert until they press in the shell`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Consent close ${closer}`, CONSENT, { comments: {} });
    const t = await postThread(d.base, artifact.id, "Check this", "#t");
    const other = await postThread(d.base, artifact.id, "And this", "#t");
    const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
    const card = page.locator(`[data-thread="${t.id}"]`);
    await card.locator(".card-head").click();
    await expect(card).toHaveClass(/selected/);
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    const threads = page.getByRole("button", { name: /^Threads/ });
    const panel = await threads.getAttribute("aria-pressed");
    await frame.locator("p").click();
    await page.keyboard.type("h");
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("button", { name: "Don't allow" })).toBeFocused();
    await page.keyboard.press(closer);
    await expect(dialog).toHaveCount(0);
    for (const k of ["s", "r", "c", "Shift+?", "j", "t"]) await page.keyboard.press(k);
    await page.waitForTimeout(300);
    await expect(dialog).toHaveCount(0);
    await expect(comment).toHaveAttribute("aria-pressed", "false");
    await expect(threads).toHaveAttribute("aria-pressed", panel!);
    await expect(card).toHaveClass(/selected/);
    await expect(page.locator(`[data-thread="${other.id}"]`)).not.toHaveClass(/selected/);
    const { threads: list } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { id: string; status: string; sent_to_agent: boolean }[] };
    expect(list.find(x => x.id === t.id)).toMatchObject({ status: "open", sent_to_agent: false });
    // The viewer's own press in the shell gives the keys back.
    await page.locator(".topbar h1").click();
    await page.keyboard.press("c");
    await expect(comment).toHaveAttribute("aria-pressed", "true");
  });
}

/** A thread selected, the viewer typing in the page, and the page's prompt
 * raised by their key, with focus on its refusing button. */
async function promptRaised(page: Page, title: string) {
  const { artifact } = await publishWith(d.base, d.token, title, CONSENT, { comments: {} });
  const t = await postThread(d.base, artifact.id, "Check this", "#t");
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const card = page.locator(`[data-thread="${t.id}"]`);
  await card.locator(".card-head").click();
  await expect(card).toHaveClass(/selected/);
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  const threads = page.getByRole("button", { name: /^Threads/ });
  const panel = await threads.getAttribute("aria-pressed");
  await frame.locator("p").click();
  await page.keyboard.type("h");
  const dialog = page.getByRole("dialog");
  const deny = dialog.getByRole("button", { name: "Don't allow" });
  await expect(deny).toBeFocused();
  /** Nothing the viewer typed acted in the shell: no mode, panel, sheet, send or resolve. */
  const unchanged = async () => {
    await page.waitForTimeout(300);
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toHaveCount(0);
    await expect(comment).toHaveAttribute("aria-pressed", "false");
    await expect(threads).toHaveAttribute("aria-pressed", panel!);
    await expect(card).toHaveClass(/selected/);
    const { threads: list } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { id: string; status: string; sent_to_agent: boolean }[] };
    expect(list.find(x => x.id === t.id)).toMatchObject({ status: "open", sent_to_agent: false });
  };
  return { dialog, deny, unchanged };
}

test("after the viewer clicks the prompt's refusing button, their typing stays inert", async ({ page }) => {
  const { dialog, deny, unchanged } = await promptRaised(page, "Consent click");
  await deny.click();
  await expect(dialog).toHaveCount(0);
  await page.keyboard.type("is it c");
  await page.keyboard.press("Shift+?");
  await unchanged();
});

test("a press on the prompt's backdrop, then Escape, leaves the typing inert", async ({ page }) => {
  const { dialog, unchanged } = await promptRaised(page, "Consent backdrop");
  await page.mouse.click(20, 300);
  await expect(dialog).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await page.keyboard.type("is it c");
  await unchanged();
});

test("Tab and Shift+Tab stay in the prompt, and after Escape the typing stays inert", async ({ page }) => {
  const { dialog, deny, unchanged } = await promptRaised(page, "Consent tab");
  await page.keyboard.press("Tab");
  await expect(deny).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(deny).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await page.keyboard.type("is it c");
  await unchanged();
});

test("form typing with Tabs and a Space while the prompt is open presses nothing outside it", async ({ page }) => {
  const { dialog, unchanged } = await promptRaised(page, "Consent form");
  for (const k of ["m", "i", "t", "h", "Tab", "1", "2", "Tab", "Space"]) await page.keyboard.press(k);
  // The Space pressed the focused "Don't allow", inside the dialog; nothing outside it.
  await expect(dialog).toHaveCount(0);
  await unchanged();
});

// The page drops focus to the shell's body mid-typing (`parent.focus()`), and
// places its pin under the pointer just before a click meant for its input.
// Neither turns the viewer's input into a press on the shell's controls.
/** A form whose page calls parent.focus() on the "m" typed in its first
 * field, then keeps the main thread busy for `busy` ms, so the viewer's next
 * keys queue up and reach the shell before any task it set. */
const form = (busy = 0) => `<!doctype html><html><head><title>Form</title></head><body><main><h2 id="t">Target</h2><input id="a"><input id="b"></main><script>
let done = false;
document.getElementById("a").addEventListener("keydown", e => { if (!done && e.key === "m") { done = true; parent.focus(); const t = performance.now(); while (performance.now() - t < ${busy}); } });
</script></body></html>`;
const FORM = form();

/** Whether the frame's own document has focus, so the viewer's keys reach the page. */
const pageHasFocus = (frame: Frame) => frame.evaluate(() => document.hasFocus());

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const busy of [0, 400]) {
    test(`${mode}${busy ? `, page busy ${busy} ms` : ""}: the viewer typing straight on after the page drops focus to the shell's body keeps their Tab and text in the page`, async ({ page }) => {
      const { artifact } = await publishWith(d.base, d.token, "Dropped focus", form(busy), {});
      const t = await postThread(d.base, artifact.id, "Check this", "#t");
      const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
      await page.locator(".thread-card").first().waitFor();
      const comment = page.getByRole("button", { name: "Comment", exact: true });
      await frame.locator("#a").click();
      // A person typing: the keys go out on a schedule, none waiting for the
      // last to be handled. The page calls parent.focus() on the "m".
      const keys = ["S", "m", "i", "t", "h", "Tab", "J", "o", "n", "e", "s"];
      const sent: Promise<void>[] = [];
      for (const k of keys) {
        sent.push(page.keyboard.press(k));
        if (busy) await page.waitForTimeout(120);
      }
      await Promise.all(sent);
      // The first key after the drop goes back with focus to the frame, whose
      // field the page gave up with the "m": the "m" and "ith" reach no field.
      // The Tab moves on from the field, and nothing reaches the shell.
      await expect(frame.locator("#b")).toHaveValue("Jones");
      expect(await frame.locator("#a").inputValue()).toBe("S");
      await expect(page.locator(".thread-card.selected")).toHaveCount(0);
      await expect(comment).toHaveAttribute("aria-pressed", "false");
      const { threads: list } = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`) as { threads: { id: string; status: string; sent_to_agent: boolean }[] };
      expect(list.find(x => x.id === t.id)).toMatchObject({ status: "open", sent_to_agent: false });
    });
  }
}

test("the viewer's own Tab or Shift+Tab out of the page stays on the shell control it reached", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Tab out", FORM, {});
  await postThread(d.base, artifact.id, "Check this", "#t");
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  for (const [field, key] of [["#b", "Tab"], ["#a", "Shift+Tab"]] as const) {
    // After shell input the gesture shield covers the frame but for a hole
    // under the pointer, so the click moves the pointer there first.
    const box = (await frame.locator(field).boundingBox())!;
    await page.mouse.move(box.x + 20, box.y + box.height / 2, { steps: 4 });
    await page.mouse.click(box.x + 20, box.y + box.height / 2);
    await expect.poll(() => frame.evaluate(f => document.activeElement?.id === f.slice(1), field)).toBe(true);
    await page.keyboard.press(key);
    await page.waitForTimeout(200);
    expect(await page.evaluate(() => document.activeElement?.localName), key).toBe("button");
    expect(await pageHasFocus(frame), key).toBe(false);
  }
});

const PLACED = `<!doctype html><html><head><title>Placed</title></head><body><main><h2 id="t">Target</h2><input id="i" style="position:absolute;left:200px;top:120px;width:200px"></main><script>
claude.use("comments").then(c => c.customAnchors({ mode(on) { document.body.dataset.mode = on; }, threads(l) { window.list = l; document.body.dataset.n = l.length; }, reveal() {} })).then(reg => {
  const h = () => window.list && window.list[0] && window.list[0].id;
  addEventListener("mousemove", e => { if (h()) reg.placed({ [h()]: { x: e.clientX + scrollX + 1, y: e.clientY + scrollY + 1 } }); });
});
</script></body></html>`;

test("a pin the page places under the pointer does not take the viewer's click", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Placed pin", PLACED, { comments: { customAnchors: true } });
  const t = await postThread(d.base, artifact.id, "Check this", "#t");
  const frame = await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  // Comment mode once, so the page holds handles it may place.
  await comment.click();
  await expect(frame.locator("body")).toHaveAttribute("data-n", /[1-9]/);
  await comment.click();
  await expect(frame.locator("body")).toHaveAttribute("data-mode", "false");
  const box = (await frame.locator("#i").boundingBox())!;
  const x = box.x + 60, y = box.y + box.height / 2;
  await page.mouse.move(x, y + 40, { steps: 3 });
  await page.mouse.move(x, y, { steps: 6 });
  await page.mouse.down();
  await page.mouse.up();
  await page.keyboard.type("is it c");
  await expect(frame.locator("#i")).toHaveValue("is it c");
  await expect(page.locator(`[data-thread="${t.id}"]`)).not.toHaveClass(/selected/);
  await expect(comment).toHaveAttribute("aria-pressed", "false");
});
