// The Content Security Policy setting on a local Clax and the notice that
// Clax's lockdown blocked a page (docs/contract.md, "Security model" and the
// bridge's `clax:blocked`): a clax at `self` whose page loads a script from
// another origin shows the notice naming that origin; its Permissions…
// opens the Permissions panel at the setting, which explains itself before
// it sends anything; once the owner turns it on and reloads, the script
// runs and no notice shows. On a hosted Clax a viewer who does not
// administer the clax sees one line, with no button. The notice sits over
// the frame, so the frame's box does not move when it shows or goes.
import { createServer as createHttpServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import type { Page } from "@playwright/test";
import { test, expect, type Daemon, api, contentFrame, openArtifact } from "./fixtures";
import { guestOn, hostedDaemon, ownerOf, shareByLink } from "./hosted-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

/** A server on another port of this host that answers `/lib.js`, a script that marks the window. */
async function scriptHost(): Promise<{ url: string; hits: () => number; close: () => void }> {
  let hits = 0;
  const s: Server = createHttpServer((_, res) => { hits++; res.writeHead(200, { "content-type": "text/javascript" }).end("window.awayLoaded = true;"); });
  await new Promise<void>(r => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${(s.address() as AddressInfo).port}`, hits: () => hits, close: () => { s.closeAllConnections(); s.close(); } };
}

const page = (away: string) => `<!doctype html><html><head><title>Blocked</title>
<script src="${away}/lib.js?key=secret"></script>
</head><body><h1>Blocked</h1></body></html>`;

const awayLoaded = (p: Page, id: string) => contentFrame(p, id, 1).then(f => f.evaluate(() => (window as unknown as { awayLoaded?: boolean }).awayLoaded ?? false));
const frameBox = async (p: Page) => (await p.locator("iframe.frame").boundingBox())!;

test("a blocked notice of a clax at self names the site, opens the setting, and goes once the owner serves it unrestricted", async ({ page: tab }) => {
  const away = await scriptHost();
  try {
    const { artifact } = await api(d.base, d.token, "/api/artifacts", {
      method: "POST", body: JSON.stringify({ title: "Blocked", files: { "index.html": { content: page(away.url), encoding: "utf8" } } }),
    }) as { artifact: { id: string } };
    await openArtifact(tab, d.base, artifact.id, 1);
    const notice = tab.locator("[data-blocked-notice]");
    await expect(notice).toContainText("This clax tried to reach another site, and Clax blocked it.");
    // Only the origin, never the path or query.
    await expect(notice.getByTestId("blocked-sites")).toHaveText(away.url);
    expect(await awayLoaded(tab, artifact.id)).toBe(false);
    expect(away.hits()).toBe(0);
    // Over the frame: the frame's box is the same with the notice and without it.
    const withNotice = await frameBox(tab);

    // Its button opens the Permissions panel at the setting, and turns nothing on.
    const patches: string[] = [];
    tab.on("request", r => { if (r.method() === "PATCH") patches.push(r.postData() ?? ""); });
    await notice.getByRole("button", { name: "Permissions…" }).click();
    const panel = tab.getByRole("dialog", { name: "Permissions for “Blocked”" });
    const sw = panel.locator("button[name=content_security_policy]");
    await expect(sw).toBeFocused();
    await expect(sw).toHaveText("Serve without Clax's Content Security Policy…");
    expect(patches).toEqual([]);

    // Its button: the explanation first, and nothing sent until the owner confirms.
    await sw.click();
    const ask = panel.getByTestId("csp-confirm");
    await expect(ask).toContainText("Clax then sends no Content-Security-Policy holding its pages to their own site: they may load code from, and send anything they can read to, any site. So may every later version, whoever publishes it: you, an editor, or an agent of either. Turn this on for a clax you trust as a website.");
    await expect(ask.getByRole("button", { name: "Confirm" })).toBeFocused();
    await expect(sw).toHaveText("Serve without Clax's Content Security Policy…");
    expect(patches).toEqual([]);
    await ask.getByRole("button", { name: "Confirm" }).click();
    await expect(sw).toHaveText("Restore Clax's Content Security Policy");
    await expect(panel.getByTestId("csp-note")).toHaveText("Served without Clax's Content Security Policy: its pages load and send where they choose.");
    expect(patches.map(p => JSON.parse(p))).toEqual([{ content_security_policy: "unrestricted" }]);
    const got = await api(d.base, d.token, `/api/artifacts/${artifact.id}`) as { artifact: { content_security_policy: string } };
    expect(got.artifact.content_security_policy).toBe("unrestricted");
    await panel.getByRole("button", { name: "Close" }).click();

    // The notice now asks for a reload; once reloaded the script runs and no notice shows.
    await expect(notice).toContainText("This clax is now served without Clax's Content Security Policy. Reload to let it reach these sites.");
    await notice.getByRole("button", { name: "Dismiss" }).click();
    await expect(notice).toHaveCount(0);
    expect(await frameBox(tab)).toEqual(withNotice);
    await tab.reload();
    await expect.poll(() => awayLoaded(tab, artifact.id)).toBe(true);
    expect(away.hits()).toBeGreaterThanOrEqual(1);
    await expect(tab.locator("[data-blocked-notice]")).toHaveCount(0);
  } finally {
    away.close();
  }
});

test("on a hosted Clax a viewer who does not administer a clax at self sees the blocked notice with no button", async ({ browser }) => {
  const away = await scriptHost();
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await api(h.base, h.token, "/api/artifacts", {
      method: "POST", body: JSON.stringify({ title: "Blocked hosted", files: { "index.html": { content: page(away.url), encoding: "utf8" } } }),
    }) as { artifact: { id: string } };
    const owner = await ownerOf(browser, origin);
    await shareByLink(owner, artifact.id, "viewer");
    const { page: guest } = await guestOn(browser, origin, artifact.id);
    const notice = guest.locator("[data-blocked-notice]");
    await expect(notice).toHaveText(/This page tried to load from other sites, and Clax blocked it\./);
    await expect(notice.getByRole("button")).toHaveText(["Dismiss"]);
    await expect(notice).not.toContainText(away.url);
    expect(away.hits()).toBe(0);
    const box = await frameBox(guest);
    await notice.getByRole("button", { name: "Dismiss" }).click();
    await expect(notice).toHaveCount(0);
    expect(await frameBox(guest)).toEqual(box);
  } finally {
    away.close();
    await h.stop();
  }
});
