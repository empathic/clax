// The `net` capability in the Clax app's view (`?app=1`, spec
// 2026-10-12-flutter-real-apps-design C6): the owner is asked once per view
// for the app's hosts in the chromeless view too; "Don't allow" stands, and
// Allow makes the daemon's grant. The declared host is under `.invalid`, so
// the daemon's fetch after a grant never reaches a network
// (`upstream_error`).
import type { Frame, Page } from "@playwright/test";
import { hostChannel, initHost } from "./app-host";
import { test, expect, type Daemon, api, contentFrame } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

const PAGE = `<!doctype html><html><head><title>Forecast</title></head><body><p>forecast</p>
<script>
(async () => {
  const net = await claude.use("net");
  window.go = () => net.fetch("https://api.clax.invalid/today").then(r => "status:" + r.status, e => e.code);
  window.ready = net !== null;
})();
</script></body></html>`;

async function openApp(page: Page, title: string, hosts = ["api.clax.invalid"]): Promise<{ id: string; frame: Frame }> {
  const { artifact } = await api(d.base, d.token, "/api/artifacts", { method: "POST", body: JSON.stringify({ title, capabilities: { net: { hosts } }, files: { "index.html": { content: PAGE, encoding: "utf8" } } }) }) as { artifact: { id: string } };
  await hostChannel(page);
  await page.goto(`${d.base}/a/${artifact.id}?app=1`);
  const frame = await contentFrame(page, artifact.id, 1);
  await initHost(page);
  await expect.poll(() => frame.evaluate(() => (window as unknown as { ready?: boolean }).ready === true)).toBe(true);
  return { id: artifact.id, frame };
}

const go = (f: Frame) => f.evaluate(() => (window as unknown as { go(): Promise<string> }).go());

test("app view: the owner is asked once for the app's hosts; Don't allow stands", async ({ page }) => {
  const { id, frame } = await openApp(page, "Forecast deny");
  const answer = go(frame);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Let “Forecast deny” use the internet?");
  await expect(dialog).toContainText("api.clax.invalid");
  await expect(dialog).toBeInViewport();
  await dialog.getByRole("button", { name: "Don't allow", exact: true }).click();
  expect(await answer).toBe("not_granted");
  expect(await go(frame)).toBe("not_granted");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const g = await api(d.base, d.token, `/api/artifacts/${id}/net-grants`) as { granted: boolean };
  expect(g.granted).toBe(false);
});

test("app view: Allow grants the app's hosts and the fetch goes to the daemon", async ({ page }) => {
  const { id, frame } = await openApp(page, "Forecast allow");
  const answer = go(frame);
  await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
  expect(await answer).toBe("upstream_error");
  const g = await api(d.base, d.token, `/api/artifacts/${id}/net-grants`) as { granted: boolean };
  expect(g.granted).toBe(true);
  // The grant stands: the next call is not asked.
  expect(await go(frame)).toBe("upstream_error");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("app view: a long host list is shown whole, scrolling, with the buttons in view", async ({ page }) => {
  const hosts = ["api.clax.invalid", ...Array.from({ length: 31 }, (_, i) => `service-${i}.region-${i}.clax.invalid`)];
  const { id, frame } = await openApp(page, "Forecast many", hosts);
  const answer = go(frame);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("service-30.region-30.clax.invalid");
  const body = dialog.locator("#prompt-body");
  expect(await body.evaluate(e => e.scrollHeight > e.clientHeight)).toBe(true);
  await expect(dialog.getByRole("button", { name: "Allow", exact: true })).toBeInViewport();
  await dialog.getByRole("button", { name: "Allow", exact: true }).click();
  expect(await answer).toBe("upstream_error");
  const g = await api(d.base, d.token, `/api/artifacts/${id}/net-grants`) as { granted: boolean; grants: unknown[] };
  expect([g.granted, g.grants.length]).toEqual([true, 32]);
});
