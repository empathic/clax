import { type Page } from "@playwright/test";
import { test, expect, type Daemon, lookedMarks, contentFrame, openArtifact, publishAs, publishNext, reach, registerSession, seenOf } from "./fixtures";
import { advance, settle } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

/** Names this page's viewer and comments on the heading through the shell's own API call, so the thread is theirs. */
async function commentAs(page: Page, aid: string, name: string, body: string): Promise<string> {
  return page.evaluate(async ([a, who, text]) => {
    await fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: who }) });
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", text); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${a}/threads`, { method: "POST", body: f })).json()).thread.id as string;
  }, [aid, name, body] as const);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  // The shell and the daemon alone decide this; the frame only shows the
  // page, the same in either mode: one mode each.
  if (mode === "sandbox") test(`${mode}: a new version puts nothing over the page: a dot, a summary line, and the Addressed group`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `cl-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Changelog ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await expect.poll(() => seenOf(page, artifact.id)).toBe(1);
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { note: "Two columns", addresses: [tid] });
    await page.getByRole("button", { name: "Reload" }).click();
    await contentFrame(page, artifact.id, 2);
    await expect(page.locator(".stage .banner:not(.notice)")).toHaveCount(0);
    await expect(page.locator(".vbtn .new")).toHaveCount(1);
    await expect(page.locator(".who .sum b.l1")).toHaveText("v2 addressed 1");
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const group = page.locator(".section-addressed");
    await expect(group.locator("h2")).toContainText("Addressed in v2");
    await expect(group.locator(".hist")).toContainText("v2 claude addressed it");
    await expect(page.locator(".thread-pin.addressed")).toHaveAttribute("data-v", "v2");
    await lookedMarks(page);
    await page.reload();
    await contentFrame(page, artifact.id, 2);
    await expect(page.locator(".section-addressed")).toHaveCount(0);
    await expect(page.locator(".vbtn .new")).toHaveCount(0);
  });

  if (mode === "subdomain") test(`${mode}: the version menu reads as a changelog, opens from the version button, and closes with Escape`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `menu-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Menu ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid], note: "Two columns" });
    await openArtifact(page, d.base, artifact.id, 2, mode);
    await page.getByRole("button", { name: /^Version 2 of 2/ }).click();
    const dialog = page.getByRole("dialog", { name: "Versions" });
    await expect(dialog.locator(".vrow").first()).toContainText("Two columns");
    await expect(dialog.locator(".vrow").first().locator(".pc")).toHaveCount(1);
    await expect(dialog.locator("a[aria-current=page]")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await page.getByRole("button", { name: /^Version 2 of 2/ }).click();
    await dialog.getByRole("link", { name: /^v1\b/ }).click();
    await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}/v/1$`));
  });

  test(`${mode}: a pinned view writes no seen mark, and a group card jumps with a highlight and resolves`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `jump-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Jump ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid] });
    // Below 900px the sidebar starts closed, so no card is on screen long
    // enough to count as looked at while the pinned view waits.
    const size = page.viewportSize()!;
    await page.setViewportSize({ width: 800, height: size.height });
    await page.goto(`${d.base}/a/${artifact.id}/v/1`);
    // The view is up, and past the time any mark would go out.
    await contentFrame(page, artifact.id, 1);
    await settle(page);
    await advance(page, 2000);
    await settle(page);
    expect(await seenOf(page, artifact.id)).toBe(1);
    await page.setViewportSize(size);
    const frame = await openArtifact(page, d.base, artifact.id, 2, mode);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.section-addressed .thread-card[data-thread="${tid}"]`);
    await reach(page, card.locator(".card-head"));
    await card.locator(".card-head").click();
    await expect(frame.locator("clax-overlay .o.flash")).toHaveCount(1);
    await card.getByRole("button", { name: "Resolve" }).click();
    await expect(card.locator(".hist")).toContainText("alex resolved");
  });
}
