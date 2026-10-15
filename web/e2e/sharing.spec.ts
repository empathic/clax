// Sharing one artifact on a hosted Clax (crates/clax-server/src/hosted.rs,
// the sharing routes; web/shell/src/ui/ShareDialog.svelte): the owner
// invites Sam, whose invite gives them their own space alone; on the
// artifact the owner opens Share, adds a GitHub login no one has signed in
// with (it waits) and Sam (a suggestion), and Sam's gallery then holds the
// artifact, with no Share of Sam's there. The owner makes it Public and
// copies its link; a signed-out browser finds it under "Public on Clax" on
// the sign-in page and opens it; made Restricted, it is gone from there.
// With CLAX_E2E_SHOTS set it writes screenshots to target/share-shots/.
import { createServer } from "node:net";
import { type Browser, type Page } from "@playwright/test";
import { NO_KEY_CONFIG, expect, publish, startDaemon, test } from "./fixtures";

const SECRET = "e2e setup secret";

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? resolve(a.port) : reject(new Error("no port"))));
    });
  });
}

async function page(browser: Browser, origin?: string): Promise<Page> {
  const ctx = await browser.newContext();
  if (origin) await ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
  return ctx.newPage();
}

/** `p` at desktop and phone width, light and dark, to `target/share-shots/<name>-<width>-<scheme>.png`. */
async function shots(p: Page, name: string) {
  if (!process.env.CLAX_E2E_SHOTS) return;
  for (const width of [1280, 390]) {
    for (const scheme of ["light", "dark"] as const) {
      await p.setViewportSize({ width, height: width > 600 ? 820 : 844 });
      await p.emulateMedia({ colorScheme: scheme });
      await p.evaluate(() => Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {}))));
      await p.screenshot({ path: `../target/share-shots/${name}-${width}-${scheme}.png` });
    }
  }
  await p.setViewportSize({ width: 1280, height: 820 });
  await p.emulateMedia({ colorScheme: "light" });
}

test("the owner shares one artifact with one person and a login, makes it public, and a signed-out browser finds it", async ({ browser }) => {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const d = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}`);
  try {
    const { artifact } = await publish(d.base, d.token, "Launch plan", { "index.html": "<title>Launch plan</title><h1>Launch plan</h1><p>Draft</p>" });
    const owner = await page(browser, origin);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);

    // Sam's invite offers no rights: Sam gets their own space.
    await owner.goto(`${origin}/people`);
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await expect(owner.locator(".invite select")).toHaveCount(0);
    await expect(owner.locator(".invite")).toContainText("They get their own space");
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    const join = (await owner.locator(".invite code").textContent())!;
    const sam = await page(browser);
    await sam.goto(join);
    await sam.getByRole("button", { name: "Join as Sam" }).click();
    await sam.waitForURL(`${origin}/`);
    await expect(sam.locator("a.card")).toHaveCount(0);

    // The owner opens Share on the artifact.
    await owner.goto(`${origin}/a/${artifact.id}`);
    await expect(owner.frameLocator("iframe.frame").locator("h1")).toHaveText("Launch plan");
    const share = owner.getByRole("button", { name: "Share", exact: true });
    await share.click();
    const dialog = owner.getByRole("dialog", { name: "Share Launch plan" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("general-note")).toHaveText("Only the people added can open it.");
    // A login no one has signed in with waits for its first sign-in.
    await dialog.getByRole("textbox", { name: "GitHub login" }).fill("@octocat");
    await dialog.getByRole("combobox", { name: "Access for the person added" }).selectOption("commenter");
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog.locator(".people li")).toHaveCount(1);
    await expect(dialog.locator(".people li").first()).toContainText("@octocat");
    await expect(dialog.locator(".people li").first()).toContainText("once they sign in with GitHub");
    // Sam, from the suggestions.
    await dialog.getByRole("textbox", { name: "GitHub login" }).fill("Sa");
    await dialog.locator(".hints button", { hasText: "Sam" }).click();
    await dialog.getByRole("combobox", { name: "Access for the person added" }).selectOption("viewer");
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog.locator(".people li")).toHaveCount(2);
    await expect(dialog.getByRole("combobox", { name: "Access for Sam" })).toHaveValue("viewer");
    await shots(owner, "dialog-people");

    // Sam's gallery now holds it; Sam may not share it.
    await sam.goto(`${origin}/`);
    await expect(sam.locator("a.card")).toHaveCount(1);
    await expect(sam.locator("a.card")).toContainText("Launch plan");
    await sam.goto(`${origin}/a/${artifact.id}`);
    await expect(sam.frameLocator("iframe.frame").locator("h1")).toHaveText("Launch plan");
    await expect(sam.locator(".topbar button.clone, .topbar .reload")).toHaveCount(0);
    await expect(sam.getByRole("button", { name: "Share", exact: true })).toHaveCount(0);

    // Public, Can view; Copy link.
    await dialog.getByRole("combobox", { name: "General access" }).selectOption("public");
    await expect(dialog.getByTestId("general-note")).toContainText("listed on this Clax's public page");
    await expect(dialog.getByRole("combobox", { name: "What anyone with the link may do" })).toHaveValue("viewer");
    await dialog.getByRole("button", { name: "Copy link" }).click();
    await expect(dialog.getByRole("button", { name: "Copied" })).toBeVisible();
    expect(await owner.evaluate(() => navigator.clipboard.readText())).toBe(`${origin}/a/${artifact.id}`);
    await shots(owner, "dialog-public");
    await dialog.getByRole("button", { name: "Done" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(share).toBeFocused();
    await shots(owner, "artifact-owner");

    // A signed-out browser finds it on the public page, and opens it.
    const anon = await page(browser);
    await anon.goto(`${origin}/`);
    await anon.waitForURL(`${origin}/signin`);
    const pub = anon.getByRole("region", { name: "Public on Clax" });
    await expect(pub.getByRole("link", { name: "Launch plan" })).toBeVisible();
    await shots(anon, "signin-public");
    await pub.getByRole("link", { name: "Launch plan" }).click();
    await anon.waitForURL(`${origin}/a/${artifact.id}`);
    await expect(anon.frameLocator("iframe.frame").locator("h1")).toHaveText("Launch plan");

    // Restricted again: gone from the public page, and closed to the link.
    await share.click();
    await owner.getByRole("dialog").getByRole("combobox", { name: "General access" }).selectOption("restricted");
    await expect(owner.getByRole("dialog").getByTestId("general-note")).toHaveText("Only the people added can open it.");
    const later = await page(browser);
    await later.goto(`${origin}/signin`);
    await expect(later.getByText("You are not signed in.")).toBeVisible();
    await expect(later.getByRole("region", { name: "Public on Clax" })).toHaveCount(0);
    const closed = await later.request.get(`${origin}/api/artifacts/${artifact.id}`);
    expect(closed.status()).toBeGreaterThanOrEqual(400);
  } finally {
    await d.stop();
  }
});
