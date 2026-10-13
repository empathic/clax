// An artifact's creator raises another person's agent on it (spec
// 2026-10-08-multi-agent-multi-person §3.4, §7.4, §17, O11), and the owner
// sees on a person's card their approvals of the owner's content (§9.5,
// O12), in a browser: Sam and Mia connect; Sam's agent publishes "Sam's
// Notes" and Mia's agent watches it; Sam, in his browser, opens the
// artifact's People panel and raises Mia's agent to publish versions there;
// Mia's agent then publishes there, and nowhere else; the owner's People
// shows the raise and who gave it, and Sam's connect-time approval of the
// owner's content. On a machine with no LAN address the daemon refuses
// invites (`not_on_lan`) and the test says so and skips.
import { type ChildProcessWithoutNullStreams, spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { api, expect, publish, startDaemon, test } from "./fixtures";
import { ownerCookie } from "./questions-helpers";

/** The environment of a CLI run as a person, on their scratch home, outside any agent channel. */
function personEnv(home: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) delete env[k];
  return env;
}

/** A person's agent: `clax mcp` on their home, driven over JSON-RPC; a
 * tool's refusal comes back as `{error}`. */
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
      if (r.result?.isError) return { error: text };
      return JSON.parse(text);
    },
    stop() { p.stdin.end(); p.kill(); },
  };
}

/** A git checkout named `name` under `home`, for an agent's derived name. */
function project(home: string, name: string) {
  const dir = join(home, "work", name);
  mkdirSync(join(dir, ".git"), { recursive: true });
  writeFileSync(join(dir, ".git", "HEAD"), "ref: refs/heads/main\n");
  return dir;
}

test("a creator raises another person's agent on their artifact from its People panel, and the owner sees it", async ({ browser }) => {
  const bin = process.env.CLAX_E2E_BIN!;
  const d = await startDaemon({ bind: "0.0.0.0" });
  const homes = { sam: mkdtempSync(join(tmpdir(), "clax-e2e-sam-")), mia: mkdtempSync(join(tmpdir(), "clax-e2e-mia-")) };
  const ownerCtx = await browser.newContext();
  const samCtx = await browser.newContext();
  const agents: Awaited<ReturnType<typeof mcpAgent>>[] = [];
  try {
    const asOwner = { cookie: ownerCookie(d.base, d.token), origin: d.base, "content-type": "application/json" };
    const links: Record<string, string> = {};
    for (const [name, home] of [["Sam", homes.sam], ["Mia", homes.mia]] as const) {
      const made = await fetch(`${d.base}/api/people/invites`, { method: "POST", headers: asOwner, body: JSON.stringify({ name }) });
      const invite = await made.json();
      test.skip(made.status === 409 && invite.error?.code === "not_on_lan", `no LAN address here: ${invite.error?.message}`);
      expect(made.status, JSON.stringify(invite)).toBe(201);
      // Sam answers yes to the owner's content reaching his agents; Mia no.
      const args = ["connect", invite.invite.command.replace(/^clax connect /, ""), "--name", `${name.toLowerCase()}-laptop`, "--no-browser", ...(name === "Sam" ? ["--yes"] : [])];
      const out = spawnSync(bin, args, { env: personEnv(home), input: "n\n", encoding: "utf8", timeout: 20_000 });
      expect(out.status, out.stderr).toBe(0);
      links[name] = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)![0];
    }
    const lan = new URL(links.Sam).origin;

    // Sam's agent publishes Sam's Notes; Mia's agent watches it.
    const sam = await mcpAgent(bin, homes.sam, project(homes.sam, "sam-notes"));
    agents.push(sam);
    const made = await sam.call("publish", { title: "Sam's Notes", html: "<main><h1>Sam's Notes</h1></main>" });
    const notes = String(made.artifact_id);
    expect(notes, JSON.stringify(made)).toMatch(/^[a-z0-9]{12}$/);
    const mia = await mcpAgent(bin, homes.mia, project(homes.mia, "notes-fix"));
    agents.push(mia);
    const watched = await mia.call("watch", { url_or_id: notes });
    expect(watched.watching, JSON.stringify(watched)).toBe(true);
    const owners = (await publish(d.base, d.token, "Quarterly Review", { "index.html": "<h1>Quarterly Review</h1>" })).artifact.id;
    // A collaborator's agent may not publish another person's artifact.
    expect(String((await mia.call("publish", { id: notes, html: "<main>Mia's fix</main>" })).error)).toContain("not_permitted");

    // Sam's browser, on his artifact: the People panel offers him a raise
    // on Mia's agent's row, for the actions he holds there.
    const s = await samCtx.newPage();
    await s.goto(links.Sam);
    await s.waitForURL(/\/people$/);
    await s.goto(`${lan}/a/${notes}`);
    await s.locator("button.who").click();
    const row = s.locator(".people .prow.agent").filter({ hasText: "notes-fix" });
    const raise = row.locator(".raise-here");
    await expect(raise.locator("summary")).toHaveText("Raise on this artifact");
    await raise.locator("summary").click();
    await expect(raise.locator("label", { hasText: "Publish new artifacts" })).toHaveCount(0);
    const box = raise.locator("label", { hasText: "Publish new versions" }).locator("input");
    await box.check();
    await expect(raise.locator("summary")).toHaveText("Raise on this artifact (raised)");
    await expect(box).toBeChecked();

    // Mia's agent may now publish Sam's Notes, and still not the owner's artifact.
    await expect.poll(async () => (await mia.call("publish", { id: notes, html: "<main>Mia's fix</main>" })).error ?? "ok").toBe("ok");
    expect(String((await mia.call("publish", { id: owners, html: "<main>no</main>" })).error)).toContain("not_permitted");

    // The owner's People: Mia's agent raised by Sam, read-only; Sam's card
    // lists his approval of the owner's content, and Mia's none.
    const o = await ownerCtx.newPage();
    await o.goto(`${d.base}/people`);
    const card = (name: string) => o.locator("article.person[data-person]").filter({ has: o.locator("h3", { hasText: new RegExp(`^${name}$`) }) });
    await expect(card("Mia").locator(".agent .note.by")).toHaveText("Raised by Sam until this session ends: Publish new versions on “Sam's Notes”");
    await expect(card("Sam").locator(".approvals.yours .line")).toHaveText("Allowed everywhere");
    await expect(card("Mia").locator(".approvals.yours")).toContainText("Not yet: their agents see your content once they allow it.");
    const people = await api(d.base, d.token, "/api/people");
    const miaAgent = people.people.find((p: { display_name: string }) => p.display_name === "Mia").agents[0];
    expect(miaAgent.elevate).toBeNull();
    expect(miaAgent.elevations.map((e: { granter: { name: string } }) => e.granter.name)).toEqual(["Sam"]);

    // Sam takes it back: Mia's agent may no longer publish there.
    await box.uncheck();
    await expect(raise.locator("summary")).toHaveText("Raise on this artifact");
    await expect.poll(async () => String((await mia.call("publish", { id: notes, html: "<main>again</main>" })).error)).toContain("not_permitted");
  } finally {
    for (const a of agents) a.stop();
    await ownerCtx.close();
    await samCtx.close();
    await d.stop();
    for (const h of Object.values(homes)) rmSync(h, { recursive: true, force: true });
  }
});
