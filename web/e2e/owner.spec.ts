// The owner identity: every browser of the person's on this machine is one
// viewer (one public ID, one name, listed once).
import type { Page } from "@playwright/test";
import { test, expect, openArtifact, publishWith, setName } from "./fixtures";

const PAGE = "<main><h2>Quarterly goals</h2></main>";

const me = (page: Page) =>
  page.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).viewer as { public_id: string; display_name: string | null });

test("two browsers on this machine are one owner, and a name set in one shows in the other at once", async ({ browser, freshDaemon: d }) => {
  const { artifact } = await publishWith(d.base, d.token, "Owner", PAGE, {});
  const chrome = await (await browser.newContext()).newPage();
  const safari = await (await browser.newContext()).newPage();
  await openArtifact(chrome, d.base, artifact.id, 1);
  await openArtifact(safari, d.base, artifact.id, 1);
  const [a, b] = [await me(chrome), await me(safari)];
  expect(a.public_id).toBe(b.public_id);
  // Both browsers report presence; the owner is listed once.
  await expect(chrome.locator(".who .ppl .tok.here")).toHaveCount(1);
  // A name set in one browser reaches the other's field without a reload.
  await setName(chrome, "Alex");
  await safari.getByRole("button", { name: "People and agents" }).click();
  const field = safari.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await expect(field).toHaveValue("Alex");
  expect((await me(safari)).display_name).toBe("Alex");
  for (const p of [chrome, safari]) await p.context().close();
});
