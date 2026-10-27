// A page is handed only the capabilities approved for its version, and the
// owner approves what a version adds from a bar over the page (plan
// 2026-10-26-quickstart-d-permissions, Decisions 9 and 15). The declared host is under `.invalid`, so the
// daemon's fetch after approving never reaches a network (`upstream_error`).
import type { Frame, Page } from "@playwright/test";
import { expect, api, contentFrame, openArtifact, publishWith, startDaemon, streamLive, test, type Daemon } from "./fixtures";
import { type Person, connect, invite, leave } from "./people-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = `<!doctype html><html><head><title>Forecast</title></head><body><p>forecast</p>
<script>
(async () => {
  const net = await claude.use("net");
  const db = await claude.use("db");
  window.fetchHost = h => net === null ? Promise.resolve("no net") : net.fetch("https://" + h + "/x").then(r => "status:" + r.status, e => e.code);
  window.r = { net: net !== null, db: db !== null, state: await (await claude.use("permissions")).state("net") };
})();
</script></body></html>`;

type R = { net: boolean; db: boolean; state: string };
/** What the page in `page`'s frame for version `n` reports, once it has run; null while it loads or reloads. */
async function reported(page: Page, id: string, n: number): Promise<R | null> {
  const f = page.frame({ url: new RegExp(`${id}\\.localhost:\\d+/_clax/v/${n}/`) });
  if (!f) return null;
  return f.evaluate(() => (window as unknown as { r?: R }).r ?? null).catch(() => null);
}
const fetchHost = (f: Frame, host: string) => f.evaluate(h => (window as unknown as { fetchHost(h: string): Promise<string> }).fetchHost(h), host);
const barText = (page: Page) => page.locator("[data-approval-bar] span");

test("an unapproved capability resolves null and appears once the owner approves", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Forecast", PAGE, { net: { hosts: ["api.clax.invalid"] }, db: {} });
  await openArtifact(page, d.base, artifact.id, 1);
  await expect.poll(() => reported(page, artifact.id, 1)).toEqual({ net: false, db: true, state: "unavailable" });
  await expect(barText(page)).toHaveText("v1 asks for: the internet (api.clax.invalid)");
  await page.getByRole("button", { name: "Review" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Let “Forecast” use these?");
  await dialog.getByRole("button", { name: "Approve", exact: true }).click();
  // The frame reloads and the page asks again.
  await expect.poll(() => reported(page, artifact.id, 1)).toMatchObject({ net: true, db: true });
  await expect(page.locator("[data-approval-bar]")).toHaveCount(0);
  const f = await contentFrame(page, artifact.id, 1);
  expect(await fetchHost(f, "api.clax.invalid")).toBe("upstream_error");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("a version that adds a host asks again and the approved host keeps working", async ({ page }) => {
  const caps1 = { net: { hosts: ["api.clax.invalid"] }, db: {} };
  const { artifact } = await publishWith(d.base, d.token, "Forecast hosts", PAGE, caps1);
  await openArtifact(page, d.base, artifact.id, 1);
  await barText(page).waitFor();
  await page.getByRole("button", { name: "Review" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Approve", exact: true }).click();
  await expect.poll(() => reported(page, artifact.id, 1)).toMatchObject({ net: true });

  await api(d.base, d.token, `/api/artifacts/${artifact.id}/versions`, {
    method: "POST",
    body: JSON.stringify({ if_version: 1, capabilities: { ...caps1, net: { hosts: ["api.clax.invalid", "api2.clax.invalid"] } }, files: { "index.html": { content: PAGE, encoding: "utf8" } } }),
  });
  await openArtifact(page, d.base, artifact.id, 2);
  await expect(barText(page)).toHaveText("v2 asks for: the internet (api2.clax.invalid)");
  await expect.poll(() => reported(page, artifact.id, 2)).toMatchObject({ net: true });
  const f = await contentFrame(page, artifact.id, 2);
  expect(await fetchHost(f, "api.clax.invalid")).toBe("upstream_error");
  expect(await fetchHost(f, "api2.clax.invalid")).toBe("not_declared");
});

test("only the owner sees the bar", async ({ browser }) => {
  const lan = await startDaemon({ bind: "0.0.0.0" });
  const ownerCtx = await browser.newContext();
  let sam: Person | undefined;
  try {
    const inv = await invite(lan, "Sam");
    test.skip("skip" in inv, "skip" in inv ? inv.skip : "");
    sam = await connect(browser, (inv as { command: string }).command, "Sam");
    const { artifact } = await publishWith(lan.base, lan.token, "Forecast owner", PAGE, { net: { hosts: ["api.clax.invalid"] } });
    // The owner sees the bar: the control.
    const owner = await ownerCtx.newPage();
    await owner.goto(`${lan.base}/a/${artifact.id}`);
    await expect(barText(owner)).toHaveText("v1 asks for: the internet (api.clax.invalid)");
    // Sam's view has loaded the artifact. The daemon withholds the approval
    // from a connected person, and the view takes the answer again once its
    // stream is live (the step that gives the owner's view its bar), so the
    // absence is judged after both.
    await sam.page.goto(`${sam.lan}/a/${artifact.id}`);
    await expect(sam.page.locator(".topbar")).toContainText("Forecast owner");
    await streamLive(sam.page);
    const seen = await sam.page.evaluate(async id => (await (await fetch(`/api/artifacts/${id}`)).json()).artifact as { may_approve?: boolean; pending_capabilities?: unknown }, artifact.id);
    expect(seen.may_approve ?? false).toBe(false);
    expect(seen.pending_capabilities ?? null).toBeNull();
    await expect(sam.page.locator("[data-approval-bar]")).toHaveCount(0);
  } finally {
    await leave(sam);
    await ownerCtx.close();
    await lan.stop();
  }
});

test("the Permissions panel revokes the internet and the page asks again", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Forecast panel", PAGE, { net: { hosts: ["api.clax.invalid"] } });
  await openArtifact(page, d.base, artifact.id, 1);
  await barText(page).waitFor();
  await page.getByRole("button", { name: "Review" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Approve", exact: true }).click();
  await expect.poll(() => reported(page, artifact.id, 1)).toMatchObject({ net: true });
  const f = await contentFrame(page, artifact.id, 1);

  await page.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "Permissions…" }).click();
  const panel = page.getByRole("dialog", { name: /Permissions for/ });
  const row = panel.locator("[data-permission=net]");
  await expect(row).toContainText("The internet (api.clax.invalid)");
  await expect(row).toContainText("Allowed since");
  await row.getByRole("button", { name: "Revoke" }).click();
  await expect(row).toContainText("Asks on first use");
  await expect(row.getByRole("button", { name: "Revoke" })).toHaveCount(0);
  await panel.getByRole("button", { name: "Close" }).click();

  // The page asks again, in the existing dialog.
  const answer = fetchHost(f, "api.clax.invalid");
  const ask = page.getByRole("dialog", { name: "Let “Forecast panel” use the internet?" });
  await expect(ask).toBeVisible();
  await ask.getByRole("button", { name: "Don't allow", exact: true }).click();
  expect(await answer).toBe("not_granted");
});
