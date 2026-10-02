import { test, expect } from "@playwright/test";
import { reach, contentFrame, openArtifact, publishWith, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PROBE = `<!doctype html><html><head><title>Probe</title></head><body><pre id="out">waiting</pre><script>
(async () => {
  let duringScript = false;
  claude.use("permissions").then(() => { duringScript = true; });
  const sync = duringScript;
  const names = ["permissions", "db", "artifact", "self", "user", "downloads", "comments", "assets", "files", "mcp", "room", "sample", "nonsense"];
  const out = { sync, sameSelf: claude.use("self") === claude.use("artifact"), prompts: 0 };
  for (const n of names) {
    const ns = await claude.use(n);
    out[n] = ns === null ? null : Object.keys(ns).sort().join(",");
    if (ns) out[n + "Frozen"] = Object.isFrozen(ns);
  }
  document.getElementById("out").textContent = JSON.stringify(out);
})();
</script></body></html>`;

const PERMS = `<!doctype html><html><head><title>Perms</title></head><body>
<button id="ask">Ask</button><pre id="out">waiting</pre><script>
(async () => {
  const perm = await claude.use("permissions");
  const out = document.getElementById("out");
  out.textContent = JSON.stringify({ comments: await perm.state("comments"), all: await perm.state() });
  document.getElementById("ask").onclick = async () => { out.textContent = JSON.stringify(await perm.request(["comments"])); };
})();
</script></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: use() resolves declared names asynchronously, frozen, and null for the rest`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Probe ${mode}`, PROBE, { db: {}, artifact: {}, comments: { composer_only: true } });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#out")).not.toHaveText("waiting");
    const out = JSON.parse(await frame.locator("#out").textContent() ?? "{}");
    expect(out).toMatchObject({
      sync: false, sameSelf: true,
      permissions: "request,state", db: "collection,doc", artifact: "edit,publish,sync", self: "edit,publish,sync",
      comments: "anchorFor,canSendToClaude,create,customAnchors,delete,onWorking,openComposer,reply,resolve,sendToClaude,working",
      user: "avatarUrl,can,canEdit,email,id,isOwner,me,name,profiles,search", downloads: null, assets: null, files: null, mcp: null, room: null, sample: null, nonsense: null,
      permissionsFrozen: true, dbFrozen: true,
    });
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test(`${mode}: permissions.request shows one dialog; a denial is final for the load, a grant persists`, async ({ page }) => {
    const { artifact } = await publishWith(d.base, d.token, `Perms ${mode}`, PERMS, { comments: {}, db: {} });
    let frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "prompt", all: { db: "granted", user: "granted", comments: "prompt" } }));
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await frame.locator("#ask").click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("post comments on this artifact under your name");
    await dialog.getByRole("button", { name: "Don't allow" }).click();
    await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "denied" }));
    await reach(page, frame.locator("#ask"));
    await frame.locator("#ask").click();
    await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "denied" }));
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page.reload();
    frame = await contentFrame(page, artifact.id, 1);
    await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "prompt", all: { db: "granted", user: "granted", comments: "prompt" } }));
    await frame.locator("#ask").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect(frame.locator("#out")).toHaveText(JSON.stringify({ comments: "granted" }));
    await page.reload();
    frame = await contentFrame(page, artifact.id, 1);
    await expect(frame.locator("#out")).toContainText('"comments":"granted"');
  });
}

test("LAN view: assets resolves null without the token", async ({ page }) => {
  const html = `<!doctype html><html><head><title>L</title></head><body><pre id="out">waiting</pre><script>
    claude.use("assets").then(a => { document.getElementById("out").textContent = a === null ? "null" : "object"; });
  </script></body></html>`;
  const { artifact } = await publishWith(d.base, d.token, "LAN assets", html, { assets: {} });
  const frame = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  await expect(frame.locator("#out")).toHaveText("null");
  const mine = await openArtifact(await page.context().newPage(), d.base, artifact.id, 1, "sandbox");
  await expect(mine.locator("#out")).toHaveText("object");
});
