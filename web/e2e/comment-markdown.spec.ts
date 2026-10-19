// Comment bodies as Markdown (docs/contract.md, "Comments and feedback"): a
// comment posted from the composer and an agent's reply render as elements,
// raw HTML in them shows as text and runs nothing, an image is a link, a
// link to another scheme is its text, a link opens in a new tab with no
// opener, and a pin's title is the comment without its syntax.
// CLAX_MD_SHOTS=<directory> also photographs the card in light and dark, at
// 1440×900 and 390×844.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { type Page } from "@playwright/test";
import { api, commentModeIn, contentFrame, expect, publishAs, registerSession, test } from "./fixtures";
import { settle } from "./time";

const PAGE = `<main><h2>Quarterly goals</h2><p>Grow revenue and keep costs flat this quarter.</p></main>`;

const COMMENT = [
  "**Two columns**, _please_ — see `grid` and ~~the old table~~.",
  "",
  "- [x] header fixed",
  "- [ ] footer",
  "",
  "| Column | Width |",
  "|:--|--:|",
  "| left | 40% |",
  "",
  "Docs: https://example.com/grid and [the guide](https://example.com/guide).",
  "![chart](https://example.com/chart.png) <img src=x onerror=\"window.__mdXss=1\"> <script>window.__mdXss=2</script>",
  "[bad](javascript:window.__mdXss=3) [also bad](&#106;avascript:window.__mdXss=4)",
].join("\n");

const REPLY = "## Done\n\nSplit into two columns:\n\n```css\n.grid { display: grid; }\n```\n\n> The footer is next.";

async function postFromComposer(page: Page, aid: string, text: string) {
  const frame = await contentFrame(page, aid, 1);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await commentModeIn(frame);
  await frame.locator("h2").hover();
  await frame.locator("h2").click();
  const composer = page.locator(".composer");
  await expect(composer).toBeVisible();
  await expect(composer).toContainText("Markdown supported");
  await composer.locator("textarea").fill(text);
  if (process.env.CLAX_MD_SHOTS) {
    mkdirSync(process.env.CLAX_MD_SHOTS, { recursive: true });
    await composer.screenshot({ path: join(process.env.CLAX_MD_SHOTS, "light-desktop-composer.png") });
  }
  await composer.getByRole("button", { name: "Post comment" }).click();
}

test("a Markdown comment and an agent's reply render as elements, with no image, script or unsafe link", async ({ page, freshDaemon }) => {
  const d = freshDaemon;
  // No request leaves the machine: example.com answers from here.
  await page.context().route(/^https:\/\/example\.com\//, r => r.fulfill({ status: 200, contentType: "text/html", body: "<title>example</title>ok" }));
  const s = await registerSession(d.base, d.token);
  const { artifact } = await publishAs(d.base, d.token, s.id, "Markdown", { "index.html": PAGE });
  await page.goto(`${d.base}/a/${artifact.id}`);
  await postFromComposer(page, artifact.id, COMMENT);

  const card = page.locator(".section-open .thread-card").first();
  const body = card.locator(".msg.you .body");
  await expect(body.locator("strong")).toHaveText("Two columns");
  await expect(body.locator("em")).toHaveText("please");
  await expect(body.locator("code")).toHaveText("grid");
  await expect(body.locator("del")).toHaveText("the old table");
  await expect(body.locator("li.task")).toHaveText(["header fixed", "footer"]);
  const boxes = body.locator("li.task input[type=checkbox]");
  await expect(boxes).toHaveCount(2);
  await expect(boxes.first()).toBeChecked();
  await expect(boxes.first()).toBeDisabled();
  await expect(boxes.nth(1)).not.toBeChecked();
  await expect(body.locator("th")).toHaveText(["Column", "Width"]);
  await expect(body.locator("td")).toHaveText(["left", "40%"]);

  // Links: the bare URL, the named one and the image, each to its own address, in a new tab.
  const links = body.locator("a");
  await expect(links).toHaveText(["https://example.com/grid", "the guide", "chart"]);
  expect(await links.evaluateAll(as => as.map(a => [a.getAttribute("href"), a.getAttribute("target"), a.getAttribute("rel")]))).toEqual([
    ["https://example.com/grid", "_blank", "noopener noreferrer nofollow"],
    ["https://example.com/guide", "_blank", "noopener noreferrer nofollow"],
    ["https://example.com/chart.png", "_blank", "noopener noreferrer nofollow"],
  ]);
  // No image, no script, no frame; the raw HTML reads as its text, and runs nothing.
  await expect(card.locator(".msg .body").locator("img, script, iframe, object, embed, svg")).toHaveCount(0);
  await expect(body).toContainText("<img src=x onerror=\"window.__mdXss=1\">");
  await expect(body).toContainText("<script>window.__mdXss=2</script>");
  await expect(body).toContainText("bad also bad");
  expect(await body.evaluate(el => [...el.querySelectorAll("*")].flatMap(e => [...e.attributes].map(a => a.name)).filter(n => n.startsWith("on")))).toEqual([]);
  expect(await page.evaluate(() => (window as { __mdXss?: number }).__mdXss ?? null)).toBeNull();

  // The agent's reply renders the same way: its heading set under the page's.
  await card.getByRole("button", { name: /^Send to / }).click();
  await expect(card.locator(".waiting")).toContainText("sent");
  const tid = (await card.getAttribute("data-thread"))!;
  await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: REPLY, author_kind: "agent" }) });
  const reply = card.locator(".msg.agent .body");
  await expect(reply.locator("h4.h2")).toHaveText("Done");
  await expect(reply.locator("pre > code")).toHaveText(".grid { display: grid; }");
  await expect(reply.locator("blockquote")).toHaveText("The footer is next.");

  // The pin's title: the comment as text, its syntax taken away.
  const pin = page.locator("button.thread-pin").first();
  await pin.hover();
  await expect(pin).toHaveAttribute("title", /^Two columns, please — see grid and the old table\.\nheader fixed\nfooter\n/);

  // A link opens a new tab that cannot reach this one.
  const [popup] = await Promise.all([page.waitForEvent("popup"), links.nth(1).click()]);
  await popup.waitForLoadState();
  expect(popup.url()).toBe("https://example.com/guide");
  expect(await popup.evaluate(() => window.opener)).toBeNull();
  await popup.close();
  await page.bringToFront();

  const dir = process.env.CLAX_MD_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of [["desktop", { width: 1440, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
      await page.setViewportSize(viewport);
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
      await card.scrollIntoViewIfNeeded();
      await settle(page);
      await page.screenshot({ path: join(dir, `${theme}-${size}-page.png`) });
      await card.screenshot({ path: join(dir, `${theme}-${size}-card.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
    }
  }
});
