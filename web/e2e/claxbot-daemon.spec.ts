// ClaxBot on a real daemon (docs/superpowers/specs/2026-10-21-claxbot-design.md
// §10, §11): no route is stubbed. The daemon drives runs against a scripted
// Messages API (fake-messages.ts) whose address `[bot] base_url` names, its
// instance key in `[bot] api_key_env`'s variable. A thread sent to ClaxBot
// from the Send menu runs on the daemon and its run panel follows the run's
// events to its end; the admin pages read and change the daemon's people,
// provider and pricing; the audit view lists the run's events.
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, openArtifact, postThread, publish, startDaemon, test } from "./fixtures";
import { fakeMessages, finishTurn, scriptTurn } from "./fake-messages";

/** A daemon of the test's own, on `fake`. */
const botDaemon = (fake: { base: string }) => startDaemon({
  config: `[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n[bot]\nbase_url = "${fake.base}"\napi_key_env = "CLAX_E2E_BOT_KEY"\n`,
  env: { CLAX_E2E_BOT_KEY: "sk-ant-e2e-not-a-real-key-0000" },
});

test("a Send to ClaxBot runs on the daemon and the run panel follows it to its end", async ({ page }) => {
  const fake = await fakeMessages([
    scriptTurn("tu_1", "Read the page", "const a = await clax.artifact.get();\nreturn a.title;", "Reading the page first."),
    finishTurn("The totals row now rounds to cents."),
  ]);
  const d = await botDaemon(fake);
  try {
    const { artifact } = await publish(d.base, d.token, "Send to ClaxBot", { "index.html": "<main><h2>Totals</h2></main>" });
    const t = await postThread(d.base, artifact.id, "The totals row should round to cents.");
    await openArtifact(page, d.base, artifact.id, 1);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.thread-card[data-thread="${t.id}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    const bot = card.getByRole("group", { name: "Clax" }).getByRole("menuitemradio", { name: /ClaxBot/ });
    await expect(bot).toContainText("Runs in Clax · instance key");
    await bot.click();
    await card.getByRole("button", { name: "Send to ClaxBot" }).click();
    const panel = page.getByRole("complementary", { name: "ClaxBot" });
    await expect(panel).toBeVisible();
    await expect(panel.locator(".st")).toHaveText("Finished");
    await expect(panel.locator(".what")).toContainText("Answering one thread · claude-sonnet-5-5");
    await expect(panel).toContainText("Reading the page first.");
    const script = panel.locator("li.script details");
    await expect(script.locator("summary")).toContainText("1 call");
    await script.locator("summary").click();
    await expect(script.getByLabel("Calls")).toContainText("artifact.get");
    await expect(panel.locator(".finish")).toContainText("The totals row now rounds to cents.");
    await expect(panel.locator(".cost")).toContainText("This run:");
    expect(fake.left()).toBe(0);
    // The finish's summary is ClaxBot's reply on the thread it answered.
    await expect(card).toContainText("The totals row now rounds to cents.");
    await expect(card).toContainText("ClaxBot");

    // The audit view lists the run's events, ClaxBot acting for the owner.
    await page.goto(`${d.base}/audit?group=bot`);
    await expect(page.getByRole("button", { name: /ClaxBot for / }).first()).toBeVisible();
  } finally {
    await d.stop();
    await fake.stop();
  }
});

test("the admin pages read and change the daemon's people, provider and pricing", async ({ page }) => {
  const fake = await fakeMessages();
  const d = await botDaemon(fake);
  try {
    await page.goto(`${d.base}/admin`);
    await expect(page.getByRole("heading", { name: "Admin", level: 2 })).toBeFocused();
    // The owner, on by default; turning the limit off for them is recorded.
    const unenforced = page.getByRole("switch", { name: /^Unenforced limit for / }).first();
    await expect(unenforced).toHaveAttribute("aria-checked", "false");
    await unenforced.click();
    await expect(unenforced).toHaveAttribute("aria-checked", "true");
    const people = await (await fetch(`${d.base}/api/admin/people`, { headers: { authorization: `Bearer ${d.token}` } })).json() as { person: string; flags: { claxbot_unenforced: boolean } }[];
    expect(people[0]).toMatchObject({ person: "*owner", flags: { claxbot_unenforced: true } });

    // The provider: its URL from config.toml (read-only), its key from the
    // environment, its models from the provider's list.
    await page.getByRole("navigation", { name: "Admin sections" }).getByRole("link", { name: "Provider" }).click();
    await expect(page.locator("#pv-base")).toHaveValue(fake.base);
    await expect(page.locator("#pv-base")).toHaveAttribute("readonly", "");
    await page.getByRole("button", { name: "Test connection" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Connected" })).toContainText("Connected in");

    // Pricing lists the seeded cost lines.
    await page.getByRole("navigation", { name: "Admin sections" }).getByRole("link", { name: "Pricing" }).click();
    await expect(page.getByRole("spinbutton", { name: /^Margin on / }).first()).toBeVisible();
  } finally {
    await d.stop();
    await fake.stop();
  }
});

test("an admin turns web on and a run searches, fetches a fixture page and lists its sources", async ({ page }) => {
  const dir = await mkdtemp(join(tmpdir(), "clax-web-fixtures-"));
  const fixtures = join(dir, "web.json");
  const PAGE = "https://docs.example.com/runes";
  await writeFile(fixtures, JSON.stringify({
    searches: { "svelte runes": [{ url: PAGE, title: "Runes", description: "State in Svelte 5" }] },
    pages: { [PAGE]: { status: 200, content_type: "text/markdown", text: "# Runes\n\n$state makes a value reactive.\n", links: [] } },
  }));
  const fake = await fakeMessages([
    scriptTurn("tu_1", "Look up runes", "const s = await clax.web.search('svelte runes');\nconst p = await clax.web.fetch(s.results[0].url);\nreturn p.text.length;", "Searching for it."),
    finishTurn("Added what runes are."),
  ]);
  const d = await startDaemon({
    config: `[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n[bot]\nbase_url = "${fake.base}"\napi_key_env = "CLAX_E2E_BOT_KEY"\n`,
    env: { CLAX_E2E_BOT_KEY: "sk-ant-e2e-not-a-real-key-0000", CLAX_TEST_WEB_FIXTURES: fixtures },
  });
  try {
    // The admin turns search and fetch on.
    await page.goto(`${d.base}/admin?tab=web`);
    const search = page.getByRole("switch", { name: "Web search" });
    const fetchSw = page.getByRole("switch", { name: "Web fetch" });
    await expect(search).toHaveAttribute("aria-checked", "false");
    await search.click();
    await fetchSw.click();
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved." })).toBeVisible();

    // A thread sent to ClaxBot runs a script that searches and fetches.
    const { artifact } = await publish(d.base, d.token, "Web run", { "index.html": "<main><h2>Runes</h2></main>" });
    const t = await postThread(d.base, artifact.id, "Add a line on what runes are.");
    await openArtifact(page, d.base, artifact.id, 1);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.thread-card[data-thread="${t.id}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    await card.getByRole("group", { name: "Clax" }).getByRole("menuitemradio", { name: /ClaxBot/ }).click();
    await card.getByRole("button", { name: "Send to ClaxBot" }).click();
    const panel = page.getByRole("complementary", { name: "ClaxBot" });
    await expect(panel.locator(".st")).toHaveText("Finished");
    const script = panel.locator("li.script details");
    await expect(script.locator("summary")).toContainText("2 calls");
    await script.locator("summary").click();
    const calls = script.getByLabel("Calls");
    const searchRow = calls.locator("li", { hasText: "web.search" });
    await expect(searchRow).toContainText("svelte runes");
    await expect(searchRow).toContainText(/\d[\d.]* credits?/);
    const fetchRow = calls.locator("li", { hasText: "web.fetch" });
    await expect(fetchRow).toContainText(PAGE);
    await expect(fetchRow).toContainText(/from search #\d+/);
    await expect(fetchRow).toContainText(/\d[\d.]* credits?/);
    // The finish lists the page it read.
    await expect(panel.locator(".finish")).toContainText("Added what runes are.");
    const link = panel.getByRole("list", { name: "Sources" }).getByRole("link", { name: "Runes" });
    await expect(link).toHaveAttribute("href", PAGE);
    expect(fake.left()).toBe(0);
  } finally {
    await d.stop();
    await fake.stop();
    await rm(dir, { recursive: true, force: true });
  }
});
