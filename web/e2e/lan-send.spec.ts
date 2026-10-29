// A LAN viewer's Send on a single-owner install (spec 2026-10-08 §10, O6):
// one press sends to the owner's newest live agent, and the button names
// it, as before people existed. The daemon listens on every interface and
// the viewer's browser reaches it at the machine's LAN address; a machine
// with none skips.
import { networkInterfaces } from "node:os";
import { asLocalViewer, expect, publishAs, registerSession, startDaemon, test } from "./fixtures";
import { threadFrom } from "./people-helpers";

const PAGE = { "index.html": "<main><h1>Quarterly Review</h1><h2>Quarterly goals</h2></main>" };
const lanAddress = () => Object.values(networkInterfaces()).flat().find(i => i && i.family === "IPv4" && !i.internal)?.address ?? null;

test("a LAN viewer of a single-owner install sends to the owner's agent in one press", async ({ browser }) => {
  const ip = lanAddress();
  test.skip(!ip, "no LAN address here");
  const d = await startDaemon({ bind: "0.0.0.0" });
  const ctx = await browser.newContext();
  try {
    const lan = `http://${ip}:${new URL(d.base).port}`;
    // The machine's own LAN address is this machine: its browser holds a
    // viewer cookie there, as a browser of this machine's does.
    await asLocalViewer(ctx, d, ip!);
    const agent = await registerSession(d.base, d.token, "claude");
    const aid = (await publishAs(d.base, d.token, agent.id, "Quarterly Review", PAGE)).artifact.id;
    const page = await ctx.newPage();
    await page.goto(`${lan}/a/${aid}`);
    const tid = await threadFrom(page, lan, aid, "The goals need a Q4 column.");
    await page.goto(`${lan}/a/${aid}?thread=${tid}`);
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    const send = card.locator(".send > button.primary:not(.caret)");
    await expect(send).toHaveText(`Send to ${agent.name}`);
    await expect(send).not.toHaveAttribute("aria-haspopup", "menu");
    await expect(card.getByRole("button", { name: "Choose the agent" })).toHaveCount(0);
    await send.click();
    await expect(card.locator(".waiting")).toHaveText(new RegExp(`^(sent to ${agent.name}|sent, waiting for the agent.*)$`));
    const t = await (await fetch(`${d.base}/api/artifacts/${aid}/threads/${tid}`, { headers: { authorization: `Bearer ${d.token}` } })).json();
    expect(t.thread.sent_to_agent).toBe(true);
  } finally {
    await ctx.close();
    await d.stop();
  }
});
