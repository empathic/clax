// The owner identity: every browser of the person's on this machine is one
// viewer (one public ID, one name, listed once), and a LAN viewer is another.
import type { Page } from "@playwright/test";
import { test, expect, openArtifact, publishWith, setName } from "./fixtures";

const PAGE = "<main><h2>Quarterly goals</h2></main>";

const me = (page: Page) =>
  page.evaluate(async () => (await (await fetch("/api/viewers/me")).json()).viewer as { public_id: string; display_name: string | null });

test("two browsers on this machine are one owner, and a name set in one shows in the other at once", async ({ browser, freshDaemon: d }) => {
  const { artifact } = await publishWith(d.base, d.token, "Owner", PAGE, {});
  const chrome = await (await browser.newContext()).newPage();
  const safari = await (await browser.newContext()).newPage();
  const lan = await (await browser.newContext()).newPage();
  await openArtifact(chrome, d.base, artifact.id, 1, "subdomain");
  await openArtifact(safari, d.base, artifact.id, 1, "subdomain");
  await openArtifact(lan, d.base, artifact.id, 1, "sandbox", { lan: true });
  const [a, b, c] = [await me(chrome), await me(safari), await me(lan)];
  expect(a.public_id).toBe(b.public_id);
  expect(c.public_id).not.toBe(a.public_id);
  // Both browsers report presence; the owner is listed once, beside the LAN viewer.
  await setName(lan, "Mia");
  await expect(chrome.locator(".who .ppl .tok.here")).toHaveCount(2);
  // A name set in one browser reaches the other's field without a reload.
  await setName(chrome, "Alex");
  await safari.getByRole("button", { name: "People and agents" }).click();
  const field = safari.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await expect(field).toHaveValue("Alex");
  expect((await me(safari)).display_name).toBe("Alex");
  // And it reaches the LAN viewer's roster as the one owner.
  await lan.getByRole("button", { name: "People and agents" }).click();
  await expect(lan.getByRole("dialog", { name: "People and agents" }).locator(".prow", { hasText: "Alex" })).toHaveCount(1);
  for (const p of [chrome, safari, lan]) await p.context().close();
});
