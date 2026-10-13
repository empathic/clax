// The inbox's approval and connection cards, and the inbox of a connected
// person's browser (spec 2026-10-08-multi-agent-multi-person §3.2, §3.3,
// §9.5, §18) in a browser. Sam connects with an invite but does not let
// the owner's content reach his agents; his agent (a real `clax mcp` on
// his scratch home) watches the owner's page; the owner sends a comment
// there to that agent by name, which waits for Sam's approval. Sam's
// browser, at the LAN address, has its own inbox: the card shows the
// owner's comment and the four decisions, and Sam allows the thread. Sam's
// agent asks Sam a question; the card waits in Sam's inbox and his answer
// reaches the agent. Kim asks to connect without an invite; the owner's
// inbox card takes the code Kim's machine shows and a name for the new
// person, and Kim's `clax connect` finishes. The daemon listens on every
// interface; on a machine with no LAN address it refuses invites
// (`not_on_lan`) and the test says so and skips.
import { type ChildProcessWithoutNullStreams, spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { expect, publish, startDaemon, test } from "./fixtures";
import { ownerCookie } from "./questions-helpers";

/** The environment of a CLI run as a person, on their scratch home, outside any agent channel. */
function personEnv(home: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) delete env[k];
  return env;
}

/** A person's agent: `clax mcp` on their home, driven over JSON-RPC. */
class Agent {
  private next = 1;
  private waiting = new Map<number, (v: { result?: { content: { text: string }[]; isError?: boolean } }) => void>();
  constructor(private p: ChildProcessWithoutNullStreams) {
    createInterface({ input: p.stdout }).on("line", l => {
      const m = JSON.parse(l) as { id?: number };
      if (m.id !== undefined) this.waiting.get(m.id)?.(m as never);
    });
  }
  static async start(bin: string, home: string, cwd: string): Promise<Agent> {
    const a = new Agent(spawn(bin, ["mcp"], { env: personEnv(home), cwd }));
    await a.rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "claude-code", version: "1" } });
    a.p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    return a;
  }
  private rpc(method: string, params: unknown) {
    const id = this.next++;
    const done = new Promise<{ result?: { content: { text: string }[]; isError?: boolean } }>(r => this.waiting.set(id, r));
    this.p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return done;
  }
  /** A tool's result object; an error result throws. */
  async call(name: string, args: unknown): Promise<Record<string, unknown>> {
    const r = await this.rpc("tools/call", { name, arguments: args });
    const text = r.result?.content[0]?.text ?? "";
    if (r.result?.isError) throw new Error(`${name}: ${text}`);
    return JSON.parse(text);
  }
  stop() { this.p.stdin.end(); this.p.kill(); }
}

test("a connected person decides an approval and answers a question in their own inbox; the owner approves a connection request", async ({ browser }) => {
  const bin = process.env.CLAX_E2E_BIN!;
  const d = await startDaemon({ bind: "0.0.0.0" });
  const sam = mkdtempSync(join(tmpdir(), "clax-e2e-sam-"));
  const kim = mkdtempSync(join(tmpdir(), "clax-e2e-kim-"));
  const ownerCtx = await browser.newContext();
  const samCtx = await browser.newContext();
  let agent: Agent | null = null;
  let kimConnect: ChildProcessWithoutNullStreams | null = null;
  try {
    const asOwner = { cookie: ownerCookie(d.base, d.token), origin: d.base, "content-type": "application/json" };
    const made = await fetch(`${d.base}/api/people/invites`, { method: "POST", headers: asOwner, body: JSON.stringify({ name: "Sam" }) });
    const invite = await made.json();
    test.skip(made.status === 409 && invite.error?.code === "not_on_lan", `no LAN address here: ${invite.error?.message}`);
    expect(made.status, JSON.stringify(invite)).toBe(201);
    // No terminal and no --yes: the owner's content does not reach Sam's agents.
    const out = spawnSync(bin, ["connect", invite.invite.command.replace(/^clax connect /, ""), "--name", "sam-laptop", "--no-browser"], { env: personEnv(sam), encoding: "utf8", timeout: 20_000 });
    expect(out.status, out.stderr).toBe(0);
    const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)![0];
    const lan = new URL(link).origin;

    // Sam's agent watches the owner's page.
    const aid = (await publish(d.base, d.token, "Quarterly Review", { "index.html": "<main><h1>Quarterly Review</h1><h2>Quarterly goals</h2></main>" })).artifact.id;
    const project = join(sam, "work", "sam-notes");
    mkdirSync(join(project, ".git"), { recursive: true });
    writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
    agent = await Agent.start(bin, sam, project);
    expect((await agent.call("watch", { url_or_id: aid })).watching).toBe(true);
    const people = await (await fetch(`${d.base}/api/people`, { headers: asOwner })).json();
    const handle = people.people.find((p: { display_name: string }) => p.display_name === "Sam").agents[0].handle as string;

    // The owner comments and sends it to Sam's agent by name: it waits for Sam.
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", "The goals need a Q4 column.\nAnd a target per team.\nAnd an owner per goal.\nAnd a due date.");
    form.set("version", "1");
    const t = await fetch(`${d.base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers: { cookie: asOwner.cookie, origin: d.base } });
    const tid = (await t.json()).thread.id as string;
    const sent = await fetch(`${d.base}/api/artifacts/${aid}/threads/${tid}/send`, { method: "POST", headers: asOwner, body: JSON.stringify({ to: handle }) });
    expect(sent.status, await sent.text()).toBe(200);

    // Sam's browser, made his by the link, has an inbox: Inbox in the header and the card.
    const s = await samCtx.newPage();
    await s.goto(link);
    await s.waitForURL(/\/people$/);
    await s.goto(`${lan}/inbox`);
    await expect(s.locator(".gbar .inbox-link")).toBeVisible();
    const card = s.locator(".acard");
    await expect(card.locator(".ahead")).toContainText(/wants to reach your agent/);
    await expect(card.locator(".what")).toContainText("1 comment on Quarterly Review «Quarterly goals»");
    // The first 3 lines, then Show all.
    await expect(card.locator(".held")).toContainText("And an owner per goal.");
    await expect(card.locator(".held")).not.toContainText("And a due date.");
    await card.getByRole("button", { name: "Show all" }).click();
    await expect(card.locator(".held")).toContainText("And a due date.");
    await expect(card.locator(".acts button")).toHaveText(["Allow this thread, including later replies", "Always allow the owner on this page", "Always allow the owner everywhere", "Decline"]);
    await card.getByRole("button", { name: "Allow this thread, including later replies" }).click();
    await expect(card.locator(".outcome")).toHaveText("Allowed in this thread");
    await expect(card).toBeFocused();

    // Sam's agent asks; the card waits in Sam's inbox, and his answer reaches it.
    const asked = agent.call("ask", { questions: [{ question: "Which layout should the review use?", header: "Layout", multi_select: false, options: [{ label: "Two columns", description: "Charts left" }, { label: "One column", description: "Stacked" }] }], timeout_s: 60 });
    const q = s.locator(".inbox .qcard");
    await expect(q).toContainText("Which layout should the review use?");
    await q.getByText("Two columns").click();
    await q.getByRole("button", { name: /^Answer/ }).click();
    const answered = await asked;
    expect(JSON.stringify(answered.reply)).toContain("Two columns");

    // Kim asks to connect without an invite; the owner approves from the inbox.
    kimConnect = spawn(bin, ["connect", `http://${new URL(lan).host.replace(/:\d+$/, "")}:${d.port}`, "--name", "kim-desktop", "--no-browser", "--yes"], { env: personEnv(kim) });
    let kimOut = "";
    kimConnect.stdout.on("data", b => { kimOut += b; });
    kimConnect.stderr.on("data", b => { kimOut += b; });
    const kimDone = new Promise<number | null>(r => kimConnect!.on("exit", r));
    await expect.poll(() => /request ([0-9A-Z]{4}-[0-9]{4})/.exec(kimOut)?.[1] ?? null, { timeout: 20_000 }).not.toBeNull();
    const code = /request ([0-9A-Z]{4}-[0-9]{4})/.exec(kimOut)![1];
    const o = await ownerCtx.newPage();
    await o.goto(`${d.base}/inbox`);
    const cc = o.locator(".ccard");
    await expect(cc.locator(".chead")).toContainText("Connection request from kim-desktop");
    await expect(cc).toContainText("key SHA256:");
    await cc.getByRole("button", { name: "Approve…" }).click();
    await expect(cc.locator('input[name="code"]')).toBeFocused();
    await expect(cc.getByRole("link", { name: "Invite" })).toHaveAttribute("href", "/people?invite");
    const approve = cc.getByRole("button", { name: "Approve", exact: true });
    await expect(approve).toBeDisabled();
    await cc.locator('input[name="code"]').fill(code);
    await expect(approve).toBeDisabled();
    await cc.locator('input[name="name"]').fill("Kim");
    await approve.click();
    await expect(cc.locator(".outcome")).toHaveText("Approved: Kim is connected");
    expect(await kimDone, kimOut).toBe(0);
    expect(kimOut).toContain("as Kim");
  } finally {
    agent?.stop();
    kimConnect?.kill();
    await ownerCtx.close();
    await samCtx.close();
    await d.stop();
    rmSync(sam, { recursive: true, force: true });
    rmSync(kim, { recursive: true, force: true });
  }
});
