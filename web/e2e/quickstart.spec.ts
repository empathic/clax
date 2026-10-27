import { readFileSync } from "node:fs";
import type { Frame } from "@playwright/test";
import { test, expect, approve, contentFrame, startDaemon } from "./fixtures";
import { SECRET, hostedDaemon } from "./hosted-helpers";

/** The attach step's prompt in the quickstart in `frame`, once the step is open. */
async function watchPrompt(frame: Frame) {
  await frame.evaluate(() => { location.hash = "attach"; });
  return frame.locator(".cmd pre", { hasText: /^Watch / });
}

/** What the framed page's `shell.address()` resolves. */
const addressIn = (frame: Frame) => frame.evaluate(async () => {
  const shell = await (window as unknown as { claude: { use(n: string): Promise<{ address(): Promise<unknown> } | null> } }).claude.use("shell");
  return shell ? shell.address() : "no shell";
});

// Each test starts a daemon of its own: it judges a brand-new gallery. Only a
// hosted site's galleries are seeded with the quickstart; a local home is not.
test("a new local home's gallery is empty, with the quickstart on", async ({ page }) => {
  const own = await startDaemon({ env: { CLAX_GALLERY__QUICKSTART: "true" } });
  try {
    await page.goto(`${own.base}/`);
    await expect(page.getByText("When an agent publishes a page, it lands here.")).toBeVisible();
    await expect(page.locator("a.card")).toHaveCount(0);
  } finally {
    await own.stop();
  }
});

test("an invited person's hosted gallery shows the quickstart, pinned", async ({ browser }) => {
  const { d, origin } = await hostedDaemon("", { CLAX_GALLERY__QUICKSTART: "true" });
  try {
    const owner = await (await browser.newContext()).newPage();
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const url = await owner.evaluate(async () => {
      const res = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Mia" }) });
      return (await res.json()).invite.url as string;
    });
    const mia = await (await browser.newContext()).newPage();
    await mia.goto(url);
    await mia.getByRole("button", { name: "Join as Mia" }).click();
    await mia.waitForURL(`${origin}/`);
    const card = mia.locator("a.card");
    await expect(card).toHaveCount(1);
    await expect(card).toContainText("Clax Quickstart");
    // Pinned shows as the Unpin button once the token is known, else as the star.
    await expect(mia.locator('.card-tools button[title="Unpin"], .card .pin')).not.toHaveCount(0);
    await expect(mia.locator(".empty-gallery")).toHaveCount(0);

    await card.click();
    const frame = mia.frameLocator("iframe").first();
    await expect(frame.locator("h1")).toHaveText("Your agents build pages. Clax gives them a home.");
    // A seeded copy ships without the ideas page, so its tab stays hidden.
    await expect(frame.locator("#tab-ideas")).toBeHidden();

    // The shell tells the framed page where it is, which its site's
    // no-referrer policy keeps from document.referrer.
    const aid = new URL(mia.url()).pathname.split("/")[2];
    const content = await contentFrame(mia, aid, 1);
    expect(await content.evaluate(() => document.referrer)).toBe("");
    expect(await addressIn(content)).toEqual({ origin, url: `${origin}/a/${aid}` });
    await expect(await watchPrompt(content)).toHaveText(`Watch ${origin}/a/${aid} for my comments.`);
  } finally {
    await d.stop();
  }
});

// The site is <ID>.localhost whichever loopback name the shell is on, so a
// shell on 127.0.0.1 tells the shell's answer apart from the page's
// localhost fallback.
test("the quickstart's watch prompt names the shell it is framed in", async ({ page }) => {
  const own = await startDaemon();
  try {
    const dir = new URL("../quickstart/", import.meta.url);
    const read = (f: string) => readFileSync(new URL(f, dir), "utf8");
    const res = await fetch(`${own.base}/api/artifacts`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${own.token}` },
      body: JSON.stringify({
        title: "Clax Quickstart",
        capabilities: JSON.parse(read("clax.json")).capabilities,
        files: Object.fromEntries(["index.html", "style.css", "ideas-data.js"].map(f => [f, { content: read(f), encoding: "utf8" }])),
      }),
    });
    expect(res.ok).toBe(true);
    const aid = ((await res.json()) as { artifact: { id: string } }).artifact.id;
    await approve(own.base, own.token, aid);
    const shell = `http://127.0.0.1:${own.port}`;
    await page.goto(`${shell}/a/${aid}`);
    const frame = await contentFrame(page, aid, 1);
    expect(await addressIn(frame)).toEqual({ origin: shell, url: `${shell}/a/${aid}` });
    await expect(await watchPrompt(frame)).toHaveText(`Watch ${shell}/a/${aid} for my comments.`);
    // Not on clax.dev: step 1 stays as it was.
    await expect(frame.locator("#toast")).toBeHidden();
  } finally {
    await own.stop();
  }
});

test("the switch off leaves the gallery empty", async ({ page }) => {
  const own = await startDaemon();
  try {
    await page.goto(`${own.base}/`);
    await expect(page.getByText("When an agent publishes a page, it lands here.")).toBeVisible();
  } finally {
    await own.stop();
  }
});
