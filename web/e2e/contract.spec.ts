// Pages written for claude.ai's runtime contract 0.2.61 (web/e2e/pages/*.html)
// run unchanged in Clax, in both frame modes. Each page is published as is,
// with the declaration its capabilities need. The checks use only what a
// viewer and the page can observe: the page's own text, the shell's roles and
// labels, downloads, and the daemon's API.
import { readdirSync, readFileSync } from "node:fs";
import { test, expect, type Frame, type Page } from "@playwright/test";
import { contentFrame, namedViewer, openArtifact, publishWith, reach, startDaemon, type FrameMode, nameField } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const dir = new URL("./pages/", import.meta.url);
const html = (file: string) => readFileSync(new URL(file, dir), "utf8");

/** Pages in web/e2e/pages that are not claude.ai sample pages but deliberate
 * misuse for artifact.spec.ts (publishing on load, publishing in a burst). */
const MISUSE = ["publish-burst.html", "publish-on-load.html"];
/** Pages that print a Clax extension's state (the `working()` capability); not plain claude.ai pages. */
const EXTENSIONS = ["working-cap.html"];
/** Pages in web/e2e/pages that stand in for an agent's page in the shell's
 * screenshots (scenes.ts): no capabilities, so no contract case. */
const SHOTS = ["sample-report.html"];

/** The content frame's URL in each frame mode: the artifact's own origin, or
 * the main origin's sandboxed `/c/` path. */
const frameUrl = (mode: FrameMode, id: string) =>
  mode === "subdomain" ? new RegExp(`^http://${id}\\.localhost:\\d+/v/1/$`) : new RegExp(`^http://localhost:\\d+/c/${id}/v/1/$`);

/** `page`, when set, makes the published page from the file (the file as
 * is, otherwise). */
type Case = { caps: Record<string, unknown>; page?(): Promise<string>; check(f: Frame, page: Page, id: string): Promise<void> };

/** Writes a document with the token, as an agent or script would. */
async function seed(id: string, path: string, data: Record<string, unknown>) {
  const res = await fetch(`${d.base}/api/artifacts/${id}/docs/${path}`, {
    method: "PUT", headers: { "content-type": "application/json", authorization: `Bearer ${d.token}` },
    body: JSON.stringify({ data }),
  });
  return res.status;
}

/** The name of the viewer the `who.html` case made for its current run. */
let wren = "";

const CASES: Record<string, Case> = {
  "permissions.html": {
    caps: { comments: {}, db: {} },
    async check(f, page) {
      await expect(f.locator("#state")).toHaveText(JSON.stringify({ db: "granted", user: "granted", comments: "prompt" }));
      await f.locator("#ask").click();
      await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
      await expect(f.locator("#state")).toHaveText(JSON.stringify({ db: "granted", user: "granted", comments: "granted" }));
    },
  },
  "tracker.html": {
    caps: { db: { rules: [{ path: "settings", write: "admin" }] }, user: {} },
    async check(f, page, id) {
      await expect(f.locator("#status")).toHaveText("tasks 2");
      await f.getByRole("textbox", { name: "New task" }).fill("Ship v1");
      await f.getByRole("button", { name: "Add" }).click();
      await expect(f.locator("#tasks li")).toHaveText(["Seeded one", "Seeded two", "Ship v1"]);
      await f.getByRole("textbox", { name: "Private note" }).fill("mine");
      await f.locator("#save-note").click();
      await expect(f.locator("#status")).toHaveText("note saved");
      await f.locator("#lock").click();
      await expect(f.locator("#status")).toHaveText("locked");
      await page.reload();
      const again = await contentFrame(page, id, 1);
      await expect(again.getByRole("textbox", { name: "Private note" })).toHaveValue("mine");
    },
  },
  "poll.html": {
    caps: { artifact: {} },
    async check(f, page, id) {
      await expect(f.locator("#count")).toHaveText("0");
      await f.locator("#vote").click();
      await expect((await contentFrame(page, id, 2)).locator("#count")).toHaveText("1");
    },
  },
  "downloads.html": {
    caps: { downloads: {} },
    async check(f, page) {
      await f.locator("#csv").click();
      const [download] = await Promise.all([page.waitForEvent("download"), page.getByRole("dialog").getByRole("button", { name: "Save" }).click()]);
      expect(download.suggestedFilename()).toBe("q3 report.csv");
      await expect(f.locator("#status")).toHaveText("saved");
      // A type outside the contract's allowlist is refused without a prompt.
      await reach(page, f.locator("#exe"));
      await f.locator("#exe").click();
      await expect(f.locator("#status")).toHaveText("rejected_extension");
      await expect(page.getByRole("dialog")).toHaveCount(0);
    },
  },
  "who.html": {
    caps: { user: { scopes: ["profile"] } },
    // Another viewer, named, whom the owner's search finds by name. The name
    // is new on every run, so a repeated run on the same daemon finds one.
    async page() {
      wren = `Wren ${Math.random().toString(36).slice(2, 10)}`;
      const other = await namedViewer(d.base, wren);
      return html("who.html").replace('data-other="u_ffffffffffffffffffffff"', `data-other="${other}"`).replace('data-find=""', `data-find="${wren}"`);
    },
    async check(f) {
      await expect(f.locator("#facts")).not.toHaveText("waiting");
      expect(JSON.parse((await f.locator("#facts").textContent())!)).toEqual({
        isOwner: true, canEdit: true, dataWrite: true, filesWrite: true, idShape: true,
        name: "", meResolved: "", isMe: true, stranger: "", other: wren, search: 1,
      });
    },
  },
  "gallery.html": {
    caps: { assets: {}, db: {} },
    async check(f, page) {
      await expect(f.locator("#status")).toHaveText("ready");
      await f.locator("#upload").click();
      await expect(f.locator("#status")).toHaveText(JSON.stringify({ loaded: 40, files: 1, type: "image/png" }));
      await reach(page, f.locator("#remove"));
      await f.locator("#remove").click();
      await expect(f.locator("#status")).toHaveText(JSON.stringify({ first: true, second: false }));
    },
  },
  "board.html": {
    caps: { comments: { customAnchors: true } },
    async check(f, page, id) {
      // create: the viewer's consent, asked once, then a write as the viewer.
      await f.locator(".note").click();
      await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
      await expect(f.locator("#status")).toHaveText("created string");
      // The strict tier: a write within 5.5 s of input to the shell is
      // refused, with nothing written. The shell input is a click in the name
      // field just before, so the refusal does not depend on how long the
      // steps since Allow took.
      await (await nameField(page)).click();
      await page.getByRole("dialog", { name: "People and agents" }).getByRole("button", { name: "Close" }).click();
      await reach(page, f.locator(".note"));
      await f.locator(".note").click();
      await expect(f.locator("#status")).toHaveText("shell_input_recent");
      // customAnchors: in comment mode the page's own click composes on its shape.
      const post = page.getByRole("button", { name: "Post comment" });
      await page.getByRole("button", { name: "Comment", exact: true }).click();
      await expect(f.locator("#status")).toHaveText("mode true");
      await reach(page, f.locator("#canvas"));
      await f.locator("#canvas").click({ position: { x: 50, y: 50 } });
      await expect(page.getByText("Red square")).toBeVisible();
      await page.locator("textarea").fill("Make it blue.");
      await post.click();
      // Out of comment mode, openComposer: the composer opens on the card; the viewer posts.
      await page.getByRole("button", { name: "Comment", exact: true }).click();
      await expect(f.locator("#status")).toHaveText("mode false");
      await reach(page, f.locator(".comment"));
      await f.locator(".comment").click();
      await expect(f.locator("#status")).toContainText('"opened":true');
      await expect(post).toBeVisible();
      await page.locator("textarea").fill("Split this card in two.");
      await post.click();
      await expect(post).toHaveCount(0);
      type Thread = { anchor: { kind: string; quote?: string | null; custom_name?: string | null }; comments: { body: string; via_page: boolean }[] };
      await expect.poll(async () => {
        const body = await (await fetch(`${d.base}/api/artifacts/${id}/threads`)).json() as { threads: Thread[] };
        return body.threads.map(t => [t.anchor.kind, t.anchor.custom_name ?? t.anchor.quote ?? "", t.comments.map(c => [c.body, c.via_page])]);
      }).toEqual([
        [expect.any(String), expect.stringContaining("Quarterly goals"), [["Looks right to me.", true]]],
        ["custom", "shape-red", [["Make it blue.", false]]],
        [expect.any(String), expect.stringContaining("Quarterly goals"), [["Split this card in two.", false]]],
      ]);
    },
  },
};

test("every sample page is a plain claude.ai page with a case here", () => {
  const files = readdirSync(dir).filter(n => n.endsWith(".html") && !MISUSE.includes(n) && !SHOTS.includes(n) && !EXTENSIONS.includes(n)).sort();
  expect(files).toEqual(Object.keys(CASES).sort());
  for (const file of files) {
    const src = html(file);
    expect(src, file).not.toMatch(/clax/i);
    expect(src, file).toMatch(/^<!doctype html>/);
    expect(src, file).toMatch(/<title>[^<]+<\/title>/);
    expect(src, file).toContain(':root:not([data-theme="light"])');
    expect(src, file).toContain(':root[data-theme="dark"]');
    expect(src, file).toContain("claude.use(");
  }
});

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const [file, c] of Object.entries(CASES)) {
    test(`${mode}: ${file} runs unchanged`, async ({ page }) => {
      const src = c.page ? await c.page() : html(file);
      const { artifact } = await publishWith(d.base, d.token, `${file} ${mode}`, src, c.caps);
      if (file === "tracker.html") {
        expect(await seed(artifact.id, "tasks/a", { title: "Seeded one", created: 1 })).toBe(200);
        expect(await seed(artifact.id, "tasks/b", { title: "Seeded two", created: 2 })).toBe(200);
      }
      const f = await openArtifact(page, d.base, artifact.id, 1, mode);
      expect(f.url()).toMatch(frameUrl(mode, artifact.id));
      await c.check(f, page, artifact.id);
    });
  }
}

test("LAN: the tracker is readable, and writable only as far as the viewer's level allows", async ({ page }) => {
  const { artifact } = await publishWith(d.base, d.token, "Tracker LAN", html("tracker.html"), CASES["tracker.html"].caps);
  expect(await seed(artifact.id, "tasks/a", { title: "Seeded one", created: 1 })).toBe(200);
  // The token without a viewer cannot write a viewer's private subtree either.
  expect(await seed(artifact.id, "data/users/u_ffffffffffffffffffffff/prefs", { note: "not yours" })).toBe(404);
  const f = await openArtifact(page, d.base, artifact.id, 1, "sandbox", { lan: true });
  expect(f.url()).toMatch(frameUrl("sandbox", artifact.id));
  await expect(f.locator("#status")).toHaveText("tasks 1");
  await f.getByRole("textbox", { name: "New task" }).fill("From the LAN");
  await f.getByRole("button", { name: "Add" }).click();
  await expect(f.locator("#status")).toHaveText("add invalid_argument");
  await f.locator("#save-note").click();
  await expect(f.locator("#status")).toHaveText("note invalid_argument");
  const name = await nameField(page);
  await name.fill("Sam");
  await Promise.all([page.waitForResponse(r => r.url().endsWith("/api/viewers/me") && r.request().method() === "PUT"), name.press("Enter")]);
  // After input to the shell the viewer's next click in the page comes after
  // a move onto it, as a hand's does (see `reach`).
  await reach(page, f.getByRole("button", { name: "Add" }));
  await f.getByRole("button", { name: "Add" }).click();
  await expect(f.locator("#tasks li")).toHaveText(["Seeded one", "From the LAN"]);
  await reach(page, f.locator("#save-note"));
  await f.locator("#save-note").click();
  await expect(f.locator("#status")).toHaveText("note saved");
  await reach(page, f.locator("#lock"));
  await f.locator("#lock").click();
  await expect(f.locator("#status")).toHaveText("lock invalid_argument");
});
