// Sharing one artifact on a hosted Clax (crates/clax-server/src/hosted.rs,
// the sharing routes; web/shell/src/ui/ShareDialog.svelte): the owner
// invites Sam, whose invite gives them their own space alone; on the
// artifact the owner opens Share, adds a GitHub login no one has signed in
// with (it waits) and Sam (a suggestion), and Sam's gallery then holds the
// artifact, with no Share of Sam's there. The public gallery is off (the
// default), so the dialog offers no Public. The owner shares it with anyone
// with the link and copies the link; a signed-out browser sees no "Public on
// Clax" on the sign-in page but opens the link; made Restricted, the link
// is closed.
// Second: the Comments switch. An app's page (a version carries an app
// bundle) shows its byline and Get in Clax; the owner turns Comments off
// in the Share dialog and shares it by link for comments, and a named
// guest finds Comment off, no New comment box, and a comment refused
// `comments_off` until the owner turns Comments on again.
// With CLAX_E2E_SHOTS set it writes screenshots to target/share-shots/, or
// to CLAX_E2E_SHOTS itself when that is an absolute directory (as under
// Bazel, which runs the tests in a copy of the project).
import { mkdirSync } from "node:fs";
import { createServer } from "node:net";
import { isAbsolute } from "node:path";
import { type Browser, type Page } from "@playwright/test";
import { NO_KEY_CONFIG, expect, publish, setName, startDaemon, test } from "./fixtures";

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
  const env = process.env.CLAX_E2E_SHOTS;
  if (!env) return;
  const dir = isAbsolute(env) ? env : "../target/share-shots";
  mkdirSync(dir, { recursive: true });
  for (const width of [1280, 390]) {
    for (const scheme of ["light", "dark"] as const) {
      await p.setViewportSize({ width, height: width > 600 ? 820 : 844 });
      await p.emulateMedia({ colorScheme: scheme });
      await p.evaluate(() => Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {}))));
      await p.screenshot({ path: `${dir}/${name}-${width}-${scheme}.png` });
    }
  }
  await p.setViewportSize({ width: 1280, height: 820 });
  await p.emulateMedia({ colorScheme: "light" });
}

test("the owner shares one artifact with one person and a login, shares it by link with no public listing, and a signed-out browser opens the link", async ({ browser }) => {
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
    // The people panel first, so the styles its chunk brings are on the page.
    await owner.locator("button.who").click();
    const panel = owner.locator('[role="dialog"].people');
    await expect(panel).toBeVisible();
    await panel.getByRole("button", { name: "Close" }).click();
    await expect(panel).toHaveCount(0);
    const share = owner.getByRole("button", { name: "Share", exact: true });
    await share.click();
    const dialog = owner.getByRole("dialog", { name: "Share Launch plan" });
    await expect(dialog).toBeVisible();
    // The login field is drawn as the other fields are: no box around it.
    const login = dialog.getByRole("textbox", { name: "GitHub login" });
    await expect(login.locator("xpath=..")).toHaveCSS("border-top-style", "none");
    await expect(login).toHaveCSS("border-radius", await dialog.getByRole("combobox", { name: "Access for the person added" }).evaluate(e => getComputedStyle(e).borderRadius));
    await expect(dialog.getByTestId("general-note")).toHaveText("Only the people added can open it.");
    // A login no one has signed in with waits for its first sign-in.
    await dialog.getByRole("textbox", { name: "GitHub login" }).fill("@octocat");
    await dialog.getByRole("combobox", { name: "Access for the person added" }).selectOption("commenter");
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog.locator(".shared li")).toHaveCount(1);
    await expect(dialog.locator(".shared li").first()).toContainText("@octocat");
    await expect(dialog.locator(".shared li").first()).toContainText("once they sign in with GitHub");
    // Sam, from the suggestions.
    await dialog.getByRole("textbox", { name: "GitHub login" }).fill("Sa");
    await dialog.locator(".hints button", { hasText: "Sam" }).click();
    await dialog.getByRole("combobox", { name: "Access for the person added" }).selectOption("viewer");
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog.locator(".shared li")).toHaveCount(2);
    await expect(dialog.getByRole("combobox", { name: "Access for Sam" })).toHaveValue("viewer");
    // The list of people stays in the dialog's flow.
    await expect(dialog.locator(".shared li").first().locator("xpath=..")).toHaveCSS("position", "static");
    await shots(owner, "dialog-people");

    // Sam's gallery now holds it; Sam may not share it.
    await sam.goto(`${origin}/`);
    await expect(sam.locator("a.card")).toHaveCount(1);
    await expect(sam.locator("a.card")).toContainText("Launch plan");
    await sam.goto(`${origin}/a/${artifact.id}`);
    await expect(sam.frameLocator("iframe.frame").locator("h1")).toHaveText("Launch plan");
    await expect(sam.locator(".topbar button.clone, .topbar .reload")).toHaveCount(0);
    await expect(sam.getByRole("button", { name: "Share", exact: true })).toHaveCount(0);

    // No Public while the gallery is off; anyone with the link, Can view; Copy link.
    const general = dialog.getByRole("combobox", { name: "General access" });
    await expect(general.locator("option")).toHaveText(["Restricted", "Anyone with the link"]);
    await general.selectOption("link");
    await expect(dialog.getByTestId("general-note")).not.toContainText("public page");
    await expect(dialog.getByRole("combobox", { name: "What anyone with the link may do" })).toHaveValue("viewer");
    await dialog.getByRole("button", { name: "Copy link" }).click();
    await expect(dialog.getByRole("button", { name: "Copied" })).toBeVisible();
    expect(await owner.evaluate(() => navigator.clipboard.readText())).toBe(`${origin}/a/${artifact.id}`);
    await shots(owner, "dialog-link");
    await dialog.getByRole("button", { name: "Done" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(share).toBeFocused();
    await shots(owner, "artifact-owner");

    // A signed-out browser sees no public listing, and opens the link.
    const anon = await page(browser);
    await anon.goto(`${origin}/`);
    await anon.waitForURL(`${origin}/signin`);
    await expect(anon.getByText("You are not signed in.")).toBeVisible();
    await expect(anon.getByRole("region", { name: "Public on Clax" })).toHaveCount(0);
    await shots(anon, "signin");
    await anon.goto(`${origin}/a/${artifact.id}`);
    await expect(anon.frameLocator("iframe.frame").locator("h1")).toHaveText("Launch plan");

    // Restricted again: closed to the link.
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

/** A thread `p` posts on version 1 of `aid`: its status and error code. */
async function postThread(p: Page, aid: string, body: string) {
  return p.evaluate(async ([id, text]) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" }));
    form.set("body", text);
    form.set("version", "1");
    const r = await fetch(`/api/artifacts/${id}/threads`, { method: "POST", body: form });
    const j = await r.json().catch(() => ({}));
    return { status: r.status, code: (j?.error?.code ?? null) as string | null };
  }, [aid, body]);
}

test("comments switch refuses a guest comment", async ({ browser }) => {
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const config = `${NO_KEY_CONFIG}[hosted]\npublic_origin = "${origin}"\nsetup_secret_env = "CLAX_E2E_SETUP_SECRET"\n`;
  const d = await startDaemon({ config, port, env: { CLAX_E2E_SETUP_SECRET: SECRET } });
  test.skip(d.port !== port, `the daemon took port ${d.port}, not ${port}`);
  try {
    const owner = await page(browser, origin);
    await owner.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
    await owner.getByLabel("Your name").fill("Ada");
    await owner.getByRole("button", { name: "Create the first person" }).click();
    await owner.waitForURL(`${origin}/`);
    const made = await publish(d.base, d.token, "Habit tracker", { "index.html": "<title>Habit tracker</title><h1>Habit tracker</h1><p>Water the plants</p>" }) as unknown as { artifact: { id: string }; version: { content_sha256: string } };
    const aid = made.artifact.id;
    // Version 1 carries an app bundle: the page is an app's.
    const put = await fetch(`${d.base}/api/artifacts/${aid}/versions/1/bundle`, {
      method: "PUT",
      headers: { authorization: `Bearer ${d.token}`, "content-type": "application/vnd.empathic.clax+zip", "clax-bundle-lineage": "l_abcdefghijklmnopqrstuvwxyz", "clax-bundle-version": "1", "clax-bundle-content-sha256": made.version.content_sha256 },
      body: "PK\u0003\u0004 a bundle the remote never reads",
    });
    expect(put.status, await put.text()).toBe(201);

    await owner.goto(`${origin}/a/${aid}`);
    await expect(owner.frameLocator("iframe.frame").locator("h1")).toHaveText("Habit tracker");
    await expect(owner.locator(".topbar .by")).toHaveText("v1 · by Ada");
    // A phone keeps an app's byline: whose app it is, its version and installs.
    await owner.setViewportSize({ width: 390, height: 844 });
    await expect(owner.locator(".topbar .by")).toBeVisible();
    await expect(owner.locator(".topbar .by")).toHaveText("v1 · by Ada");
    await owner.setViewportSize({ width: 1280, height: 820 });
    await expect(owner.getByRole("link", { name: "Get in Clax" })).toHaveAttribute("href", `/a/${aid}/get`);
    const comment = owner.locator("button.comment");
    await expect(comment).toBeEnabled();

    // Comments off, and the page shared by link for comments.
    await owner.getByRole("button", { name: "Share", exact: true }).click();
    const dialog = owner.getByRole("dialog", { name: "Share Habit tracker" });
    const sw = dialog.getByRole("switch", { name: "Comments" });
    await expect(sw).toBeChecked();
    await expect(dialog.getByTestId("comments-note")).toHaveText("People who can comment may comment.");
    await dialog.locator("label.sw").click();
    await expect(sw).not.toBeChecked();
    await expect(dialog.getByTestId("comments-note")).toHaveText("Off: nobody can comment, whatever their access.");
    await dialog.getByRole("combobox", { name: "General access" }).selectOption("link");
    await dialog.getByRole("combobox", { name: "What anyone with the link may do" }).selectOption("commenter");
    await expect(dialog.getByTestId("general-note")).toHaveText("Anyone with the link can open it, without signing in.");
    await shots(owner, "dialog-comments-off");
    await dialog.getByRole("button", { name: "Done" }).click();
    // The owner's own view follows at once.
    await expect(comment).toBeDisabled();
    await expect(comment).toHaveAttribute("title", "Comments are off for this page");
    // The owner, who may share it, sees Get in Clax as the bar's other actions.
    await expect(owner.getByRole("link", { name: "Get in Clax" })).toHaveClass(/ghost/);
    await shots(owner, "owner-app");

    // A named guest with the link: Comment is off, no New comment box, and a comment is refused.
    const guest = await page(browser);
    await guest.goto(`${origin}/a/${aid}`);
    await expect(guest.frameLocator("iframe.frame").locator("h1")).toHaveText("Habit tracker");
    await setName(guest, "Mia");
    const theirs = guest.locator("button.comment");
    await expect(theirs).toBeDisabled();
    await expect(theirs).toHaveAttribute("title", "Comments are off for this page");
    const threads = guest.locator("button.threads");
    await expect(threads).toHaveAttribute("aria-pressed", /^(true|false)$/);
    if ((await threads.getAttribute("aria-pressed")) === "false") await threads.click();
    await expect(guest.locator("aside.sidebar")).toBeVisible();
    await expect(guest.locator("aside.sidebar .empty-open")).toHaveText("Comments are off for this page.");
    await expect(guest.locator(".new-comment-open")).toHaveCount(0);
    // Polled: the guest's name may still be on its way (401 guest_name_required until it lands).
    await expect.poll(() => postThread(guest, aid, "while comments are off")).toEqual({ status: 403, code: "comments_off" });
    await shots(guest, "guest-comments-off");

    // On again: the guest's comment lands.
    await owner.getByRole("button", { name: "Share", exact: true }).click();
    await owner.getByRole("dialog").locator("label.sw").click();
    await expect(owner.getByRole("dialog").getByRole("switch", { name: "Comments" })).toBeChecked();
    await expect(comment).toBeEnabled();
    await expect.poll(() => postThread(guest, aid, "comments are on")).toEqual({ status: 201, code: null });

    // Off again with that thread open: the sidebar says why its card has no reply box.
    await owner.getByRole("dialog").locator("label.sw").click();
    await expect(owner.getByRole("dialog").getByRole("switch", { name: "Comments" })).not.toBeChecked();
    await guest.reload();
    await expect(guest.locator("button.threads")).toHaveAttribute("aria-pressed", /^(true|false)$/);
    if ((await guest.locator("button.threads").getAttribute("aria-pressed")) === "false") await guest.locator("button.threads").click();
    await expect(guest.locator("aside.sidebar .section-open [data-thread]").first()).toBeVisible();
    await expect(guest.locator("aside.sidebar .section-open .no-reply")).toHaveText("Comments are off for this page.");
    await shots(guest, "guest-comments-off-thread");
  } finally {
    await d.stop();
  }
});
