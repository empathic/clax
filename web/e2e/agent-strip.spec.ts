// The inbox's agent strip, thread owners, the `withheld` delivery state and
// the Send menu by agent name grouped by person (spec
// 2026-10-08-multi-agent-multi-person §10, §13, §17) in a browser. The
// owner has two agents on a page, one of them asking a question; Sam
// connects without letting the owner's content reach his agents, and his
// agent (a real `clax mcp` on his scratch home) watches the page. The
// owner's inbox opens with the strip, and its chip filters the inbox by
// that agent. On the page, the owner's Send menu lists "Your agents", then
// "Sam's agents", where Sam's says he approves first; the thread sent to
// Sam's agent reads "waiting for Sam's approval" and shows its owner chip
// with Sam's name. Sam's browser, at the LAN address, shows his approval
// card above that thread; he allows it there. The daemon listens on every
// interface; on a machine with no LAN address it refuses invites
// (`not_on_lan`) and the test says so and skips.
import { type ChildProcessWithoutNullStreams, spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { api, expect, publishAs, registerSession, startDaemon, test } from "./fixtures";
import { ask, ownerCookie } from "./questions-helpers";

/** The environment of a CLI run as a person, on their scratch home, outside any agent channel. */
function personEnv(home: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) delete env[k];
  return env;
}

/** A person's agent: `clax mcp` on their home, driven over JSON-RPC. */
async function mcpAgent(bin: string, home: string, cwd: string) {
  const p: ChildProcessWithoutNullStreams = spawn(bin, ["mcp"], { env: personEnv(home), cwd });
  const waiting = new Map<number, (v: { result?: { content: { text: string }[]; isError?: boolean } }) => void>();
  createInterface({ input: p.stdout }).on("line", l => { const m = JSON.parse(l) as { id?: number }; if (m.id !== undefined) waiting.get(m.id)?.(m as never); });
  let next = 1;
  const rpc = (method: string, params: unknown) => {
    const id = next++;
    const done = new Promise<{ result?: { content: { text: string }[]; isError?: boolean } }>(r => waiting.set(id, r));
    p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return done;
  };
  await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "claude-code", version: "1" } });
  p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
  return {
    async call(name: string, args: unknown): Promise<Record<string, unknown>> {
      const r = await rpc("tools/call", { name, arguments: args });
      const text = r.result?.content[0]?.text ?? "";
      if (r.result?.isError) throw new Error(`${name}: ${text}`);
      return JSON.parse(text);
    },
    stop() { p.stdin.end(); p.kill(); },
  };
}

test("the inbox groups by agent, threads name their owners and wait for approval, and the Send menu groups agents by person", async ({ browser }) => {
  const bin = process.env.CLAX_E2E_BIN!;
  const d = await startDaemon({ bind: "0.0.0.0" });
  const sam = mkdtempSync(join(tmpdir(), "clax-e2e-sam-"));
  const ownerCtx = await browser.newContext();
  const samCtx = await browser.newContext();
  const lanCtx = await browser.newContext();
  let agent: Awaited<ReturnType<typeof mcpAgent>> | null = null;
  try {
    const asOwner = { cookie: ownerCookie(d.base, d.token), origin: d.base, "content-type": "application/json" };
    const made = await fetch(`${d.base}/api/people/invites`, { method: "POST", headers: asOwner, body: JSON.stringify({ name: "Sam" }) });
    const invite = await made.json();
    test.skip(made.status === 409 && invite.error?.code === "not_on_lan", `no LAN address here: ${invite.error?.message}`);
    expect(made.status, JSON.stringify(invite)).toBe(201);
    const out = spawnSync(bin, ["connect", invite.invite.command.replace(/^clax connect /, ""), "--name", "sam-laptop", "--no-browser"], { env: personEnv(sam), encoding: "utf8", timeout: 20_000 });
    expect(out.status, out.stderr).toBe(0);
    const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)![0];
    const lan = new URL(link).origin;

    await fetch(`${d.base}/api/viewers/me`, { method: "PUT", headers: asOwner, body: JSON.stringify({ display_name: "Alex" }) });
    // The owner's two agents on the page: claude publishes it, codex watches it.
    const claude = await registerSession(d.base, d.token, "claude");
    const codex = await registerSession(d.base, d.token, "codex");
    const aid = (await publishAs(d.base, d.token, claude.id, "Quarterly Review", { "index.html": "<main><h1>Quarterly Review</h1><h2>Quarterly goals</h2></main>" })).artifact.id;
    await api(d.base, d.token, `/api/sessions/${codex.id}/watches/${aid}`, { method: "PUT", session: codex.id });
    // Sam's agent, named after its repository, watches it too.
    const project = join(sam, "work", "sam-notes");
    mkdirSync(join(project, ".git"), { recursive: true });
    writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
    agent = await mcpAgent(bin, sam, project);
    expect((await agent.call("watch", { url_or_id: aid })).watching).toBe(true);
    // claude asks the owner: it waits on the owner.
    await ask(d.base, d.token, claude.id, { source: "ask", artifact_id: aid, questions: [{ question: "Two columns or one?", header: "Layout" }] });

    // The owner's inbox opens with the strip; claude's chip filters the inbox by claude.
    const o = await ownerCtx.newPage();
    await o.goto(`${d.base}/inbox`);
    const chip = o.locator(".astrip .achip").first();
    await expect(chip).toHaveText(/^claude · waiting on you (under a minute|\d+ min) \(1 question\)$/);
    await chip.click();
    await expect(o).toHaveURL(/\/inbox\?agent=a_/);
    await expect(o.locator('.astrip .achip[aria-pressed="true"]')).toContainText("claude");

    // The owner comments on the page; the Send menu groups the agents by person.
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", "The goals need a Q4 column.");
    form.set("version", "1");
    const tid = (await (await fetch(`${d.base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers: { cookie: asOwner.cookie, origin: d.base } })).json()).thread.id as string;
    await o.goto(`${d.base}/a/${aid}?thread=${tid}`);
    const card = o.locator(`.thread-card[data-thread="${tid}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    const menu = card.getByRole("menu");
    await expect(menu.getByRole("group")).toHaveCount(2);
    await expect(menu.locator(".send-group-h")).toHaveText(["Your agents", "Sam's agents"]);
    expect((await menu.getByRole("group", { name: "Your agents" }).locator(".send-name").allTextContents()).sort()).toEqual(["claude", "codex"]);
    const samItem = menu.getByRole("group", { name: "Sam's agents" }).getByRole("menuitemradio");
    await expect(samItem.locator(".send-name")).toHaveText("sam-notes");
    await expect(samItem.locator(".send-first")).toHaveText("Sam approves first");
    await samItem.click();
    // Chosen, the button names Sam's agent with Sam.
    await card.getByRole("button", { name: "Send to sam-notes (Sam's)" }).click();
    // Sent, it waits for Sam, and names its owner with Sam's name.
    await expect(card.locator(".waiting")).toHaveText("waiting for Sam's approval");
    await expect(card.locator(".owner-chip")).toHaveText("sam-notes · Sam");
    await expect(card.locator(".owner-chip")).toHaveAttribute("title", "sam-notes, Sam's agent");

    // A LAN viewer has no agents: Send opens the menu, which names each
    // agent's person and never who approves first (its browser never asks).
    const t2 = (await (await fetch(`${d.base}/api/artifacts/${aid}/threads`, { method: "POST", body: (() => { const f = new FormData(); f.set("anchor", form.get("anchor") as string); f.set("body", "Another note."); f.set("version", "1"); return f; })(), headers: { cookie: asOwner.cookie, origin: d.base } })).json()).thread.id as string;
    const l = await lanCtx.newPage();
    const refused: number[] = [];
    l.on("response", r => { if (r.url().includes("/approves-first")) refused.push(r.status()); });
    await l.goto(`${lan}/a/${aid}?thread=${t2}`);
    const lanCard = l.locator(`.thread-card[data-thread="${t2}"]`);
    const lanSend = lanCard.locator(".send > button.primary:not(.caret)");
    await expect(lanSend).toHaveText("Send to …");
    await lanSend.click();
    const lanMenu = lanCard.getByRole("menu");
    await expect(lanMenu.locator(".send-group-h")).toHaveCount(2);
    expect((await lanMenu.locator(".send-group-h").allTextContents()).sort()).toEqual(["Alex's agents", "Sam's agents"]);
    await expect(lanMenu.locator(".send-person")).toHaveCount(3);
    expect(refused).toEqual([]);
    await expect(lanMenu.locator(".send-first")).toHaveCount(0);
    await expect(l.locator(".thread-approval")).toHaveCount(0);
    // The thread waiting for Sam reads only that it was sent, to whom, to a
    // LAN viewer (owner decision 2026-10-14, "Only \"sent\"").
    await expect(l.locator(`.thread-card[data-thread="${tid}"] .waiting`)).toHaveText("sent to sam-notes");

    // Sam's browser: his approval card above the thread, and his agent by name only.
    const s = await samCtx.newPage();
    await s.goto(link);
    await s.waitForURL(/\/people$/);
    await s.goto(`${lan}/a/${aid}?thread=${tid}`);
    const samCard = s.locator(`.thread-card[data-thread="${tid}"]`);
    const above = s.locator(".thread-approval .acard");
    await expect(above).toContainText("The goals need a Q4 column.");
    expect(await above.evaluate((a, id) => a.closest(".thread-approval")!.nextElementSibling?.getAttribute("data-thread") === id, tid)).toBe(true);
    await expect(samCard.locator(".waiting")).toHaveText("waiting for your approval");
    await expect(samCard.locator(".owner-chip")).toHaveText("sam-notes");
    await above.getByRole("button", { name: "Allow this thread, including later replies" }).click();
    await expect(samCard.locator(".waiting")).not.toHaveText(/approval/);
  } finally {
    agent?.stop();
    await ownerCtx.close();
    await samCtx.close();
    await lanCtx.close();
    await d.stop();
    rmSync(sam, { recursive: true, force: true });
  }
});
