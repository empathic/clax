// Tags in the web UI (docs/contract.md "Tags"): the owner makes them on
// /tags and tags an artifact from its page; the gallery shows them on its
// cards and filters by them (match all, kept in the address); the inbox
// filters by them; a second page follows each change live; deleting a tag
// takes it off everything; a viewer who may not tag sees the tags and no
// controls. With CLAX_E2E_SHOTS set to a directory, the last test
// photographs the gallery, the Tags page, the artifact page with its tag
// editor open and the inbox's Tag filter, desktop and phone, light and dark.
import { mkdirSync } from "node:fs";
import { type Page } from "@playwright/test";
import { type Daemon, api, expect, openArtifact, publish, publishAs, registerSession, streamLive, test } from "./fixtures";
import { hostedDaemon, openAt, ownerOf } from "./hosted-helpers";

const PAGE = { "index.html": "<main><h2>Board</h2><p>Numbers</p></main>" };
const SHOTS = process.env.CLAX_E2E_SHOTS;

type Tag = { id: string; name: string };
const makeTag = async (d: Daemon, name: string, colour: string, description?: string) =>
  ((await api(d.base, d.token, "/api/tags", { method: "POST", body: JSON.stringify({ name, colour, description }) })) as { tag: Tag }).tag;
const tagArtifact = (d: Daemon, aid: string, tags: string[]) => api(d.base, d.token, `/api/artifacts/${aid}/tags`, { method: "PUT", body: JSON.stringify({ tags }) });
const cards = (p: Page) => p.locator("a.card h3");
const card = (p: Page, title: string) => p.locator(".card-wrap", { has: p.getByRole("heading", { name: title }) });
const chips = (p: Page, title: string) => card(p, title).locator(".ctags .tag");
const titleChips = (p: Page) => p.locator(".topbar .ttags a.tag");

test("the owner makes tags on /tags, tags an artifact from its page, and the gallery filters by them, every tag chosen, in its address", async ({ page, freshDaemon: d }) => {
  const plan = (await publish(d.base, d.token, "Launch plan", PAGE)).artifact.id;
  const budget = (await publish(d.base, d.token, "Budget", PAGE)).artifact.id;
  await publish(d.base, d.token, "Roadmap", PAGE);

  // The owner makes a tag with the form: a name, a colour and a description.
  await page.goto(`${d.base}/tags`);
  await expect(page.getByText("There are no tags yet. Make one with New tag.")).toBeVisible();
  await page.getByRole("button", { name: "New tag" }).click();
  const form = page.getByRole("form", { name: "New tag" });
  await expect(form.getByLabel("Name")).toBeFocused();
  await form.getByLabel("Name").fill("q4");
  await form.getByRole("radio", { name: "Blue" }).check();
  await form.getByLabel("Description").fill("Fourth quarter");
  await form.getByRole("button", { name: "Create tag" }).click();
  const row = page.locator(".trow", { hasText: "Fourth quarter" });
  await expect(row.locator("a.tag")).toHaveText("q4");
  await expect(row.locator("a.tag")).toHaveAttribute("data-c", "blue");
  await expect(row).toContainText("Blue · 0 artifacts");
  // Focus goes back to the button that opened the form.
  await expect(page.getByRole("button", { name: "New tag" })).toBeFocused();
  // A name the list has, in any case, is refused in the form.
  await page.getByRole("button", { name: "New tag" }).click();
  await form.getByLabel("Name").fill("Q4");
  await form.getByRole("button", { name: "Create tag" }).click();
  await expect(form.getByRole("alert")).toContainText("exists");
  await form.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("button", { name: "New tag" })).toBeFocused();
  await page.getByRole("button", { name: "Edit tag q4" }).click();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Edit tag q4" })).toBeFocused();
  await makeTag(d, "design", "purple");

  // From the artifact's page: Add tags opens the editor on its search; the
  // keyboard checks a tag, a click another; Escape closes it.
  await page.goto(`${d.base}/a/${plan}`);
  await page.getByRole("button", { name: "Add tags" }).click();
  const editor = page.getByRole("dialog", { name: "Edit tags" });
  await expect(editor.getByLabel("Find a tag")).toBeFocused();
  await page.keyboard.type("q4");
  // Enter checks the only tag the search shows, once the list has loaded.
  await expect(editor.getByRole("checkbox", { name: "q4" })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(titleChips(page)).toHaveText(["q4"]);
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await page.keyboard.press("ArrowDown");
  await expect(editor.getByRole("checkbox", { name: "design" })).toBeFocused();
  await page.keyboard.press("Space");
  await expect(titleChips(page)).toHaveText(["design", "q4"]);
  await expect(editor.getByRole("checkbox", { name: "q4" })).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Edit tags" })).toBeFocused();
  // A tag beside the title opens the gallery filtered by it.
  await expect(titleChips(page).first()).toHaveAttribute("href", "/?tag=design");
  await tagArtifact(d, budget, ["q4"]);

  // The gallery: each card's tags; the bar lists the tags in use.
  await page.goto(`${d.base}/`);
  await expect(chips(page, "Launch plan")).toHaveText(["design", "q4"]);
  await expect(chips(page, "Budget")).toHaveText(["q4"]);
  await expect(chips(page, "Roadmap")).toHaveCount(0);
  const bar = page.getByRole("group", { name: "Filter by tags" });
  await expect(bar.locator("button.tag")).toHaveText(["design", "q4"]);
  await expect(bar.getByRole("link", { name: "Manage tags" })).toHaveAttribute("href", "/tags");
  // Pressing tags filters to the artifacts carrying every one, in the address.
  await bar.getByRole("button", { name: "Filter by tag q4" }).click();
  await expect(page).toHaveURL(`${d.base}/?tag=q4`);
  await expect(cards(page)).toHaveCount(2);
  await bar.getByRole("button", { name: "Filter by tag design" }).click();
  await expect(page).toHaveURL(`${d.base}/?tag=q4&tag=design`);
  await expect(cards(page)).toHaveText(["Launch plan"]);
  // It survives a reload, and back and forward walk it.
  await page.reload();
  await expect(cards(page)).toHaveText(["Launch plan"]);
  await expect(bar.getByRole("button", { name: "Filter by tag design" })).toHaveAttribute("aria-pressed", "true");
  await page.goBack();
  await expect(page).toHaveURL(`${d.base}/?tag=q4`);
  await expect(cards(page)).toHaveCount(2);
  await page.goForward();
  await expect(cards(page)).toHaveText(["Launch plan"]);
  await bar.getByRole("button", { name: "Clear" }).click();
  await expect(cards(page)).toHaveCount(3);
  // A card's tag adds it to the filter; the search matches tag names too.
  await card(page, "Budget").getByRole("button", { name: "Filter by tag q4" }).click();
  await expect(page).toHaveURL(`${d.base}/?tag=q4`);
  await bar.getByRole("button", { name: "Clear" }).click();
  await page.getByRole("searchbox", { name: "Search artifacts" }).fill("desig");
  await expect(cards(page)).toHaveText(["Launch plan"]);
});

test("a second page follows tag changes live, and deleting a tag takes it off every artifact", async ({ page, context, freshDaemon: d }) => {
  const plan = (await publish(d.base, d.token, "Launch plan", PAGE)).artifact.id;
  const q4 = await makeTag(d, "q4", "blue");
  await tagArtifact(d, plan, ["q4"]);
  await page.goto(`${d.base}/`);
  await expect(chips(page, "Launch plan")).toHaveText(["q4"]);
  const view = await context.newPage();
  await view.goto(`${d.base}/a/${plan}`);
  await expect(titleChips(view)).toHaveText(["q4"]);
  await streamLive(page);
  await streamLive(view);

  // Tagged elsewhere: both pages show it.
  await makeTag(d, "design", "purple");
  await api(d.base, d.token, `/api/artifacts/${plan}/tags`, { method: "PATCH", body: JSON.stringify({ add: ["design"] }) });
  await expect(chips(page, "Launch plan")).toHaveText(["design", "q4"]);
  await expect(titleChips(view)).toHaveText(["design", "q4"]);
  // The gallery filters by both, one named by its ID in the address: it
  // names both by name.
  const design = ((await api(d.base, d.token, "/api/tags")) as { tags: Tag[] }).tags.find(t => t.name === "design")!;
  await page.goto(`${d.base}/?tag=q4&tag=${design.id}`);
  await expect(page).toHaveURL(`${d.base}/?tag=q4&tag=design`);
  await expect(cards(page)).toHaveText(["Launch plan"]);
  await streamLive(page);
  // Renamed and recoloured: the artifact's page hears it too, and the
  // filter follows it, the address naming it anew in place.
  const depth = await page.evaluate(() => history.length);
  await api(d.base, d.token, `/api/tags/${q4.id}`, { method: "PATCH", body: JSON.stringify({ name: "Q4 plan", colour: "green" }) });
  await expect(titleChips(view)).toHaveText(["design", "Q4 plan"]);
  await expect(titleChips(view).last()).toHaveAttribute("data-c", "green");
  await expect(chips(page, "Launch plan")).toHaveText(["design", "Q4 plan"]);
  await expect(page).toHaveURL(`${d.base}/?tag=Q4+plan&tag=design`);
  await expect(cards(page)).toHaveText(["Launch plan"]);
  expect(await page.evaluate(() => history.length)).toBe(depth);

  // Deleting on the Tags page asks first, saying how many carry it.
  const tags = await context.newPage();
  await tags.goto(`${d.base}/tags`);
  const asked = new Promise<string>(resolve => tags.once("dialog", dlg => { resolve(dlg.message()); void dlg.accept(); }));
  await tags.getByRole("button", { name: "Delete tag design" }).click();
  expect(await asked).toBe('Delete the tag "design"? It is on 1 artifact; deleting it removes it from it.');
  await expect(tags.getByRole("status")).toHaveText("Deleted design, and took it off 1 artifact.");
  await expect(tags.locator(".trow")).toHaveCount(1);
  await expect(tags.getByRole("button", { name: "Edit tag Q4 plan" })).toBeFocused();
  await expect(chips(page, "Launch plan")).toHaveText(["Q4 plan"]);
  await expect(titleChips(view)).toHaveText(["Q4 plan"]);
  // The deleted tag drops out of the filter.
  await expect(page).toHaveURL(`${d.base}/?tag=Q4+plan`);
  await expect(cards(page)).toHaveText(["Launch plan"]);
  await view.close();
  await tags.close();
});

test("the tag editor shows a refusal past twenty tags and closes on a press outside; +N, and N tags on a phone, list them for anyone", async ({ page, browser, freshDaemon: d }) => {
  const plan = (await publish(d.base, d.token, "Launch plan", PAGE)).artifact.id;
  const names = Array.from({ length: 21 }, (_, i) => `tag ${String(i + 1).padStart(2, "0")}`);
  for (const n of names) await makeTag(d, n, "teal");
  await tagArtifact(d, plan, names.slice(0, 20));
  await page.goto(`${d.base}/a/${plan}`);
  await expect(titleChips(page)).toHaveCount(20);
  // Not every chip fits: "+N" lists them all.
  const more = page.locator(".topbar .tmore");
  await expect(more).toHaveText(/^\+\d+/);
  await more.click();
  const list = page.getByRole("dialog", { name: "Tags" });
  await expect(list.locator(".tag")).toHaveCount(20);
  await page.keyboard.press("Escape");
  await expect(list).toHaveCount(0);
  await expect(more).toBeFocused();
  // A twenty-first is refused, saying why, and its box unchecks.
  await page.getByRole("button", { name: "Edit tags" }).click();
  const editor = page.getByRole("dialog", { name: "Edit tags" });
  const last = editor.getByRole("checkbox", { name: "tag 21" });
  await last.click();
  await expect(editor.getByRole("alert")).toContainText("20");
  await expect(last).not.toBeChecked();
  await expect(titleChips(page)).toHaveCount(20);
  // A press outside closes it.
  await page.locator(".topbar h1").click();
  await expect(editor).toHaveCount(0);

  // On a phone, the title reads "20 tags", which lists them. A local
  // daemon's browser on loopback is the owner, who may tag, once the shell
  // has its token; a viewer who may not is the hosted test below.
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
  const phone = await ctx.newPage();
  await openArtifact(phone, d.base, plan, 1);
  const count = phone.getByRole("button", { name: "All 20 tags" });
  await expect(count).toHaveText("20 tags");
  await expect(phone.locator(".topbar .tedit")).toHaveCount(1);
  await count.click();
  await expect(phone.getByRole("dialog", { name: "Tags" }).locator(".tag")).toHaveCount(20);
  if (SHOTS) {
    mkdirSync(SHOTS, { recursive: true });
    for (const theme of ["light", "dark"] as const) {
      await phone.emulateMedia({ colorScheme: theme });
      await phone.screenshot({ path: `${SHOTS}/artifact-list-phone-${theme}.png` });
    }
    await page.setViewportSize({ width: 1280, height: 820 });
    await page.getByRole("button", { name: "All 20 tags" }).click();
    await page.screenshot({ path: `${SHOTS}/artifact-list-desktop-light.png` });
  }
  await ctx.close();
});

// A viewer who may not tag: a hosted daemon's person who may read the page
// (a local daemon frames pages for the owner alone, who may tag).
test("a viewer who may not tag sees an artifact's tags and no controls, here or on the Tags page", async ({ browser }) => {
  const { d, origin } = await hostedDaemon();
  try {
    const plan = (await publish(d.base, d.token, "Launch plan", PAGE)).artifact.id;
    await makeTag(d, "q4", "blue", "Fourth quarter");
    await tagArtifact(d, plan, ["q4"]);
    const owner = await ownerOf(browser, origin);
    const join = await owner.evaluate(async id => {
      const r = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Sam", rights: [{ type: "clax-artifacts", actions: ["read"], locations: [id] }] }) });
      return (await r.json()).invite.url as string;
    }, plan);
    const page = await (await browser.newContext()).newPage();
    await page.goto(join);
    await page.getByRole("button", { name: "Join as Sam" }).click();
    await page.waitForURL(`${origin}/`);
    await openAt(page, origin, plan, 1);
    await expect(titleChips(page)).toHaveText(["q4"]);
    await expect(page.locator(".topbar .tedit")).toHaveCount(0);
    await page.goto(`${origin}/tags`);
    await expect(page.locator(".trow")).toContainText("Fourth quarter");
    await expect(page.getByRole("button", { name: "New tag" })).toHaveCount(0);
    await expect(page.locator(".trow button")).toHaveCount(0);
    await page.goto(`${origin}/`);
    await expect(page.getByRole("group", { name: "Filter by tags" }).getByRole("link", { name: "All tags" })).toBeVisible();
  } finally {
    await d.stop();
  }
});

test("the inbox's Tags filter keeps the items about artifacts carrying every tag picked, in its address", async ({ page, freshDaemon: d }) => {
  const s = await registerSession(d.base, d.token);
  const alpha = (await publishAs(d.base, d.token, s.id, "Alpha page", PAGE)).artifact.id;
  const beta = (await publishAs(d.base, d.token, s.id, "Beta page", PAGE)).artifact.id;
  const q4 = await makeTag(d, "q4", "blue");
  await makeTag(d, "design", "purple");
  await tagArtifact(d, alpha, ["q4", "design"]);
  await tagArtifact(d, beta, ["q4"]);
  await page.goto(`${d.base}/inbox`);
  const inbox = page.locator(".inbox");
  await expect(inbox.getByRole("button", { name: /^Alpha page/ })).toBeVisible();
  await expect(inbox.getByRole("button", { name: /^Beta page/ })).toBeVisible();
  const pick = inbox.getByRole("button", { name: "Tags Any tag" });
  await pick.click();
  const dialog = inbox.getByRole("dialog", { name: "Filter by tags" });
  await dialog.getByRole("checkbox", { name: "design" }).check();
  await expect(page).toHaveURL(`${d.base}/inbox?tag=design`);
  await expect(inbox.getByRole("button", { name: /^Beta page/ })).toHaveCount(0);
  await expect(inbox.getByRole("button", { name: /^Alpha page/ })).toBeVisible();
  await dialog.getByRole("checkbox", { name: "q4" }).check();
  await expect(page).toHaveURL(`${d.base}/inbox?tag=design&tag=q4`);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await page.reload();
  await expect(inbox.getByRole("button", { name: "Tags design +1" })).toBeVisible();
  await expect(inbox.getByRole("button", { name: /^Alpha page/ })).toBeVisible();
  await expect(inbox.getByRole("button", { name: /^Beta page/ })).toHaveCount(0);
  // A rename keeps filtering, the address naming the tag anew; the list follows.
  await streamLive(page);
  await api(d.base, d.token, `/api/tags/${q4.id}`, { method: "PATCH", body: JSON.stringify({ name: "Q4 plan" }) });
  await expect(page).toHaveURL(`${d.base}/inbox?tag=design&tag=Q4+plan`);
  await expect(inbox.getByRole("button", { name: /^Alpha page/ })).toBeVisible();
  await inbox.getByRole("button", { name: "Tags design +1" }).click();
  await expect(inbox.getByRole("dialog", { name: "Filter by tags" }).getByRole("checkbox", { name: "Q4 plan" })).toBeChecked();
});

test("screenshots of the tags in the gallery, the Tags page, the artifact page and the inbox", async ({ page, freshDaemon: d }) => {
  test.skip(!SHOTS, "set CLAX_E2E_SHOTS to a directory to take them");
  test.setTimeout(120_000);
  mkdirSync(SHOTS!, { recursive: true });
  const s = await registerSession(d.base, d.token);
  const pub = async (title: string) => (await publishAs(d.base, d.token, s.id, title, PAGE)).artifact.id;
  const ids = { launch: await pub("Launch plan for the autumn release"), budget: await pub("Budget"), roadmap: await pub("Roadmap"), notes: await pub("Interview notes") };
  const all = [["q4", "blue", "Fourth quarter work"], ["design", "purple", "Needs a designer's eye"], ["urgent", "red", null], ["research", "teal", "From user interviews"], ["finance", "green", null], ["draft", "gray", "Not ready to share"], ["legal", "brown", null], ["marketing", "orange", "Campaign material"], ["ideas", "yellow", null], ["people", "pink", null]] as const;
  for (const [n, c, desc] of all) await makeTag(d, n, c, desc ?? undefined);
  await tagArtifact(d, ids.launch, ["q4", "design", "urgent", "marketing", "draft"]);
  await tagArtifact(d, ids.budget, ["q4", "finance"]);
  await tagArtifact(d, ids.notes, ["research", "people", "ideas"]);
  const shoot = async (name: string, open?: () => Promise<void>) => {
    for (const [size, viewport] of [["desktop", { width: 1280, height: 820 }], ["phone", { width: 390, height: 844 }]] as const) {
      for (const theme of ["light", "dark"] as const) {
        await page.setViewportSize(viewport);
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.reload();
        await open?.();
        await page.screenshot({ path: `${SHOTS}/${name}-${size}-${theme}.png` });
        expect(await page.evaluate(() => document.documentElement.scrollWidth), `${name} ${size} scrolls sideways`).toBeLessThanOrEqual(viewport.width);
      }
    }
  };
  await page.goto(`${d.base}/?tag=q4`);
  await shoot("gallery-filtered", () => expect(page.locator("a.card")).toHaveCount(2));
  await page.goto(`${d.base}/`);
  await shoot("gallery", () => expect(page.locator(".ctags").first()).toBeVisible());
  await page.goto(`${d.base}/tags`);
  await shoot("tags-page", () => expect(page.locator(".trow")).toHaveCount(10));
  await shoot("tags-page-edit", async () => {
    await page.getByRole("button", { name: "Edit tag design" }).click();
    await expect(page.getByRole("form", { name: "Edit tag design" })).toBeVisible();
  });
  await page.goto(`${d.base}/a/${ids.launch}`);
  await shoot("artifact", () => expect(titleChips(page)).toHaveCount(5));
  await shoot("artifact-picker", async () => {
    await page.locator(".topbar .tedit").click();
    await expect(page.getByRole("dialog", { name: "Edit tags" }).getByRole("checkbox")).toHaveCount(10);
  });
  await page.goto(`${d.base}/inbox?tag=q4`);
  await shoot("inbox-filter", async () => {
    await expect(page.getByRole("searchbox", { name: "Search the inbox" })).toBeVisible();
    if (await page.getByRole("button", { name: "Filters" }).isVisible()) await page.getByRole("button", { name: "Filters" }).click();
    await page.getByRole("button", { name: "Tags q4" }).click();
    await expect(page.getByRole("dialog", { name: "Filter by tags" })).toBeVisible();
  });
});
