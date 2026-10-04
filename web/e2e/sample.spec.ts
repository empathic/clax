import { readFileSync } from "node:fs";
import { type Frame, type Page } from "@playwright/test";
import { test, expect, type Daemon, STUB_CONFIG, contentFrame, openArtifact, publishWith, reach, startDaemon } from "./fixtures";

const PAGE = readFileSync(new URL("./pages/sample.html", import.meta.url), "utf8");

test.describe("with the stub provider", () => {
  // The stub provider is a daemon setting: these tests get a daemon of their own.
  let d: Daemon;
  test.beforeAll(async () => { d = await startDaemon({ config: STUB_CONFIG }); });
  test.afterAll(async () => { await d?.stop(); });

  async function open(page: Page, title: string, mode: "subdomain" | "sandbox"): Promise<Frame> {
    const { artifact } = await publishWith(d.base, d.token, title, PAGE, { sample: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#status")).toHaveText("ready");
    return f;
  }

  /** Fills the prompt, clicks `button`, and answers the consent dialog with `consent` (every test's first call asks). */
  async function ask(page: Page, f: Frame, prompt: string, button = "#ask", consent: "Allow" | "Don't allow" = "Allow") {
    await f.locator("#prompt").fill(prompt);
    await f.locator(button).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Anthropic API key");
    await dialog.getByRole("button", { name: consent, exact: true }).click();
  }

  for (const mode of ["subdomain", "sandbox"] as const) {
    test(`${mode}: consent once, progressive text, and the call count`, async ({ page }) => {
      const f = await open(page, `Sample ${mode}`, mode);
      await expect(page.locator(".sample-count")).toHaveText("0 Claude calls today");
      await ask(page, f, "tell me about three bears");
      await expect(f.locator("#note")).toHaveText("done");
      await expect(f.locator("#out")).toHaveText("echo: tell me about three bears");
      expect(Number(await f.locator("#updates").textContent())).toBeGreaterThan(1);
      await expect(page.locator(".sample-count")).toHaveText("1 Claude call today");
      // After the dialog, the shell covers the page until the pointer moves there (fixtures.ts `reach`).
      await reach(page, f.locator("#ask"));
      await f.locator("#prompt").fill("another question");
      await f.locator("#ask").click();
      await expect(f.locator("#note")).toHaveText("done");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.locator(".sample-count")).toHaveText("2 Claude calls today");
      expect(JSON.parse(await f.locator("#limits").textContent() ?? "null")).toEqual({ maxPromptBytes: 65536, tools: { maxCount: 16 } });
    });

    test(`${mode}: consent lasts for the view: a reload asks again`, async ({ page }) => {
      const { artifact } = await publishWith(d.base, d.token, `Sample reload ${mode}`, PAGE, { sample: {} });
      const f = await openArtifact(page, d.base, artifact.id, 1, mode);
      await expect(f.locator("#status")).toHaveText("ready");
      await ask(page, f, "first");
      await expect(f.locator("#note")).toHaveText("done");
      await page.reload();
      const again = await contentFrame(page, artifact.id, 1);
      await expect(again.locator("#status")).toHaveText("ready");
      await ask(page, again, "second");
      await expect(again.locator("#note")).toHaveText("done");
    });

    test(`${mode}: a declined viewer gets not_granted without asking again`, async ({ page }) => {
      const f = await open(page, `Sample deny ${mode}`, mode);
      await ask(page, f, "q", "#ask", "Don't allow");
      await expect(f.locator("#note")).toHaveText("not_granted");
      await reach(page, f.locator("#ask"));
      await f.locator("#ask").click();
      await expect(f.locator("#note")).toHaveText("not_granted");
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });

    test(`${mode}: a page tool runs and its result reaches the answer`, async ({ page }) => {
      const f = await open(page, `Sample tool ${mode}`, mode);
      await ask(page, f, "[[tool:getColor]]", "#tool");
      await expect(f.locator("#note")).toHaveText("done");
      await expect(f.locator("#tool-runs")).toHaveText("1");
      await expect(f.locator("#out")).toHaveText("Checking.\n\ntool said: teal");
    });

    test(`${mode}: Stop rejects cancelled and keeps the partial text`, async ({ page }) => {
      const f = await open(page, `Sample stop ${mode}`, mode);
      await ask(page, f, "[[slow]]");
      await expect(f.locator("#out")).toContainText("tick");
      await reach(page, f.locator("#stop"));
      await f.locator("#stop").click();
      await expect(f.locator("#note")).toHaveText("cancelled");
      await expect(f.locator("#kept")).toContainText("tick");
    });
  }
});

test.describe("without a key", () => {
  let d: Daemon;
  test.beforeEach(({ daemon }) => { d = daemon; });

  for (const mode of ["subdomain", "sandbox"] as const) {
    test(`${mode}: use("sample") resolves null and the header shows no count`, async ({ page }) => {
      const { artifact } = await publishWith(d.base, d.token, `No key ${mode}`, PAGE, { sample: {} });
      const f = await openArtifact(page, d.base, artifact.id, 1, mode);
      await expect(f.locator("#status")).toHaveText("unavailable");
      await expect(page.locator(".sample-count")).toHaveCount(0);
    });
  }
});

test.describe("a LAN viewer, with the stub provider", () => {
  // The stub provider is a daemon setting: these tests get a daemon of their own.
  let d: Daemon;
  test.beforeAll(async () => { d = await startDaemon({ config: STUB_CONFIG }); });
  test.afterAll(async () => { await d?.stop(); });

  test("use(\"sample\") resolves null: no dialog, no count, nothing spent", async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, "Sample LAN", PAGE, { sample: {} });
    const asked: string[] = [];
    page.on("request", r => { if (r.url().includes("/sample")) asked.push(r.url()); });
    const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
    await expect(f.locator("#status")).toHaveText("unavailable");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".sample-count")).toHaveCount(0);
    expect(asked).toEqual([]);
  });
});
