import { test, expect, type Daemon, reach, contentFrame, openArtifact, publishWith } from "./fixtures";
import { guestOn, hostedDaemon, ownerOf, shareByLink } from "./hosted-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

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

test(`use() resolves declared names asynchronously, frozen, and null for the rest`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Probe`, PROBE, { db: {}, artifact: {}, comments: { composer_only: true } });
  const frame = await openArtifact(page, d.base, artifact.id, 1);
  await expect(frame.locator("#out")).not.toHaveText("waiting");
  const out = JSON.parse(await frame.locator("#out").textContent() ?? "{}");
  expect(out).toMatchObject({
    sync: false, sameSelf: true,
    permissions: "request,state", db: "artifact,collection,doc", artifact: "edit,publish,sync", self: "edit,publish,sync",
    comments: "anchorFor,canSendToClaude,create,customAnchors,delete,onWorking,openComposer,reply,resolve,sendToClaude,working",
    user: "avatarUrl,can,canEdit,email,id,isOwner,me,name,profiles,search", downloads: null, assets: null, files: null, mcp: null, room: null, sample: null, nonsense: null,
    permissionsFrozen: true, dbFrozen: true,
  });
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test(`permissions.request shows one dialog; a denial is final for the load, a grant persists`, async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, `Perms`, PERMS, { comments: {}, db: {} });
  let frame = await openArtifact(page, d.base, artifact.id, 1);
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

// A viewer who is not the owner: a hosted daemon's guest on a page shared by
// link (a local daemon frames pages for the owner alone).
test("a guest's view: assets resolves null without the token", async ({ page, browser }) => {
  const html = `<!doctype html><html><head><title>L</title></head><body><pre id="out">waiting</pre><script>
    claude.use("assets").then(a => { document.getElementById("out").textContent = a === null ? "null" : "object"; });
  </script></body></html>`;
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await publishWith(h.base, h.token, "Guest assets", html, { assets: {} });
    await shareByLink(await ownerOf(browser, origin), artifact.id);
    const { frame } = await guestOn(browser, origin, artifact.id);
    await expect(frame.locator("#out")).toHaveText("null");
  } finally {
    await h.stop();
  }
  // The owner's own view has it.
  const { artifact } = await publishWith(d.base, d.token, "Own assets", html, { assets: {} });
  const mine = await openArtifact(page, d.base, artifact.id, 1);
  await expect(mine.locator("#out")).toHaveText("object");
});
