import { readFileSync } from "node:fs";
import { test, expect, type Daemon, namedViewer, openArtifact, publishWith, nameField } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const html = (name: string) => readFileSync(new URL(`./pages/${name}`, import.meta.url), "utf8");
const facts = async (f: import("@playwright/test").Frame) => {
  // Read until it holds facts: a reloading document starts over at "waiting".
  let out: Record<string, unknown> | null = null;
  await expect.poll(async () => {
    const t = await f.locator("#facts").textContent().catch(() => null);
    try { out = t === null || t === "waiting" ? null : JSON.parse(t); } catch { out = null; }
    return out !== null;
  }).toBe(true);
  return out!;
};

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: user in the owner shell`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Who ${mode}`, html("who.html"), { user: { scopes: ["profile"] }, db: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    expect(await facts(f)).toMatchObject({ isOwner: true, canEdit: true, dataWrite: true, filesWrite: true, idShape: true, name: "", isMe: true, stranger: "", other: null, search: 0 });
    const name = await nameField(page);
    await name.fill("Alex");
    await Promise.all([page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"), name.press("Enter")]);
    await f.locator("#refresh").click();
    await expect(f.locator("#greeting")).toHaveText("Hello, Alex");
    await expect.poll(async () => (await facts(f)).search).toBe(1);
    expect(await facts(f)).toMatchObject({ name: "Alex", meResolved: "Alex", search: 1 });
  });

  test(`${mode}: assets upload, display, list, and delete`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Gallery ${mode}`, html("gallery.html"), { assets: {}, db: {} });
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("#status")).toHaveText("ready");
    await f.locator("#upload").click();
    await expect(f.locator("#status")).toHaveText(JSON.stringify({ loaded: 40, files: 1, type: "image/png" }));
    await f.locator("#remove").click();
    await expect(f.locator("#status")).toHaveText(JSON.stringify({ first: true, second: false }));
  });
}

test("LAN: user is not the owner, resolves other viewers by name, and can write data from the next document once named; assets resolves null", async ({ page }) => {
  const other = await namedViewer(d.base, "Ärger Ölund");
  const who = html("who.html").replace('data-other="u_ffffffffffffffffffffff"', `data-other="${other}"`);
  const { artifact } = await publishWith(d.base, d.token, "Who LAN", who, { user: { scopes: ["profile"] }, db: {} });
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  expect(await facts(f)).toMatchObject({ isOwner: false, canEdit: false, dataWrite: false, filesWrite: false, idShape: true, search: 0, other: "Ärger Ölund", stranger: "" });
  const name = await nameField(page);
  await name.fill("Sam");
  await Promise.all([page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"), name.press("Enter")]);
  // can() is fixed for the life of the frame's document: naming does not change it.
  await f.locator("#refresh").click();
  await expect.poll(async () => (await facts(f)).name).toBe("Sam");
  expect(await facts(f)).toMatchObject({ name: "Sam", dataWrite: false });
  // The next document is given the named viewer's level.
  await f.evaluate(() => location.reload()).catch(() => { /* the reload ends this document mid-call */ });
  await expect.poll(async () => (await facts(f)).dataWrite).toBe(true);
  expect(await facts(f)).toMatchObject({ name: "Sam", other: "Ärger Ölund", isOwner: false });
  const { artifact: g } = await publishWith(d.base, d.token, "Gallery LAN", html("gallery.html"), { assets: {} });
  const page2 = await page.context().newPage();
  const f2 = await openArtifact(page2, d.base, g.id, 1, "sandbox", { lan: true });
  await expect(f2.locator("#status")).toHaveText("read-only");
});
