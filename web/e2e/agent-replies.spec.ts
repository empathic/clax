// Agents' replies as people read them (spec 2026-10-08-multi-agent-multi-
// person §11 "Display"): the owner and a connected person (Sam) each read their own
// person's agent's reply as `<name> (agent, via <harness>)` and the other's
// as `<name> (<person>'s agent, via <harness>)`, in a thread's card and in
// the inbox. The daemon listens on every interface, so its invites name
// this machine's LAN address; on a machine with none the test skips.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { api, expect, publish, registerSession, setName, startDaemon, test } from "./fixtures";

/** Runs the CLI as the person, on their scratch home, outside any agent channel. */
function asPerson(home: string, args: string[], input?: string, cwd?: string, env: NodeJS.ProcessEnv = {}) {
  const all: NodeJS.ProcessEnv = { ...process.env, CLAX_HOME: home, ...env };
  for (const k of ["CLAX_CHANNEL", "CLAUDECODE", "CODEX_SESSION_ID", "CLAX_PORT"]) if (!(k in env)) delete all[k];
  return spawnSync(process.env.CLAX_E2E_BIN!, args, { env: all, input, cwd, encoding: "utf8", timeout: 20_000 });
}

/** A thread on `aid` made in `page`'s browser (its viewer), sent to the agents. */
async function threadFrom(page: Page, aid: string, body: string): Promise<string> {
  return page.evaluate(async ([a, b]) => {
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    form.set("body", b);
    form.set("version", "1");
    const res = await fetch(`/api/artifacts/${a}/threads`, { method: "POST", body: form });
    if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
    const id = (await res.json()).thread.id as string;
    const sent = await fetch(`/api/artifacts/${a}/threads/${id}/send`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    if (!sent.ok) throw new Error(`${sent.status} ${await sent.text()}`);
    return id;
  }, [aid, body]);
}

test("each reader reads its own person's agent's reply as its agent's, and another's with that person", async ({ browser }) => {
  const d = await startDaemon({ bind: "0.0.0.0" });
  const sam = mkdtempSync(join(tmpdir(), "clax-e2e-sam-"));
  const ownerCtx = await browser.newContext();
  const personCtx = await browser.newContext();
  try {
    const owner = await ownerCtx.newPage();
    await owner.goto(`${d.base}/people`);
    await owner.getByRole("button", { name: "Invite a person" }).click();
    await owner.getByRole("textbox", { name: "Name", exact: true }).fill("Sam");
    await owner.getByRole("button", { name: "Create invite" }).click();
    const made = owner.locator(".invite code").or(owner.locator('.invite [role="alert"]'));
    await expect(made).toBeVisible();
    const alert = owner.locator('.invite [role="alert"]');
    const refused = (await alert.count()) ? await alert.textContent() : null;
    test.skip(!!refused && refused.includes("LAN"), `no LAN address here: ${refused}`);
    const command = (await owner.locator(".invite code").textContent())!;
    const out = asPerson(sam, ["connect", command.replace(/^clax connect /, ""), "--name", "sam-laptop", "--yes", "--no-browser"]);
    expect(out.status, out.stderr).toBe(0);
    const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)![0];
    // Sam's agent, named after its checkout (sam-notes), and the owner's.
    const project = join(sam, "work", "sam-notes");
    mkdirSync(join(project, ".git"), { recursive: true });
    writeFileSync(join(project, ".git", "HEAD"), "ref: refs/heads/main\n");
    const hook = asPerson(sam, ["hook", "--agent", "claude", "session-start"], JSON.stringify({ session_id: "e2e-sam-1", cwd: project, hook_event_name: "SessionStart", source: "startup" }), project);
    expect(hook.status, hook.stderr).toBe(0);
    const mine = await registerSession(d.base, d.token, "claude");

    const aid = (await publish(d.base, d.token, "Plan", { "index.html": "<main><h2>Quarterly goals</h2></main>" })).artifact.id;
    await owner.goto(`${d.base}/a/${aid}`);
    await setName(owner, "Alex");
    const first = await threadFrom(owner, aid, "For the owner's agent");
    const second = await threadFrom(owner, aid, "For Sam's agent");
    // The owner's agent replies to the first; Sam's, through its MCP
    // server on Sam's machine, to the second.
    await api(d.base, d.token, `/api/artifacts/${aid}/threads/${first}/comments`, { method: "POST", session: mine.id, body: JSON.stringify({ body: "Done here", author_kind: "agent" }) });
    const rpc = [
      { jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "0" } } },
      { jsonrpc: "2.0", method: "notifications/initialized" },
      { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "comments_reply", arguments: { url_or_id: aid, thread_id: second, text: "Done there" } } },
    ].map(m => JSON.stringify(m)).join("\n") + "\n";
    const shim = asPerson(sam, ["mcp", "--agent", "claude"], rpc, project, { CLAUDE_CODE_SESSION_ID: "e2e-sam-1", CLAX_CHANNEL: "agent" });
    expect(shim.stdout, shim.stderr).toContain('\\"replied\\": true');
    const name = (await api(d.base, d.token, "/api/agents")).agents.find((a: { handle: string; harness: string; person: string | null }) => a.person === null && a.handle && a.harness === "claude")?.name ?? mine.name;

    const card = (p: Page, body: string) => p.locator(".thread-card").filter({ hasText: body });
    await owner.reload();
    await expect(card(owner, "Done here").locator(".by .author").last()).toHaveText(`${name} (agent, via claude)`);
    await expect(card(owner, "Done there").locator(".by .author").last()).toHaveText("sam-notes (Sam's agent, via claude)");

    const person = await personCtx.newPage();
    await person.goto(link);
    await person.waitForURL(/\/people$/);
    await person.goto(new URL(`/a/${aid}`, link).href);
    await expect(card(person, "Done here").locator(".by .author").last()).toHaveText(`${name} (Alex's agent, via claude)`);
    await expect(card(person, "Done there").locator(".by .author").last()).toHaveText("sam-notes (agent, via claude)");

    // The owner's inbox names each agent as its other items do: its own by
    // name, Sam's with Sam, under Plan's heading.
    await owner.goto(`${d.base}/inbox`);
    const plan = owner.locator(`.igroup[data-group="${aid}"]`);
    await expect(plan.locator(".gtitle")).toHaveText(/^Plan(, \d+ new)?$/);
    await expect(plan.locator(".title", { hasText: new RegExp(`^${name} replied$`) })).toHaveCount(1);
    await expect(plan.locator(".title", { hasText: /^sam-notes \(Sam's agent\) replied$/ })).toHaveCount(1);
  } finally {
    await ownerCtx.close();
    await personCtx.close();
    rmSync(sam, { recursive: true, force: true });
    await d.stop();
  }
});
