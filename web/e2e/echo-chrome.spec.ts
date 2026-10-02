// Echo's chrome in a real browser: the favicon, the theme before first
// paint, and the shell's keys, which belong to the page while it has focus
// and to a dialog while one is open (spec §8, "Keys"; decisions Q2 and Q6).
import { expect, test, type Page } from "@playwright/test";
import { api, openArtifact, postThread, publishWith, startDaemon } from "./fixtures";

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

// The attack: a page raises its consent prompt while the viewer types for it,
// so their next keys land on the prompt's button in the shell. None of them
// may send, resolve, or change the view.
const CONSENT = `<!doctype html><html><head><title>Consent</title></head><body><main><h2 id="t">Target</h2><p>Type here.</p></main><script>
claude.use("permissions").then(perm => {
  let asked = false;
  addEventListener("keydown", () => { if (!asked) { asked = true; perm.request(["comments"]); } });
});
</script></body></html>`;

test("a consent prompt raised while the viewer types takes the keys: S, R, C, T and ? do nothing", async ({ page }) => {
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
