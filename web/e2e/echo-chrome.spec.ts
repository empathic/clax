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
