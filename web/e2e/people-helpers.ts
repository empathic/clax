// Two people in a browser test (spec 2026-10-08-multi-agent-multi-person):
// invites, `clax connect` from scratch homes, the browser link at the
// daemon's LAN address, and each person's agents as real `clax mcp` shims
// on their home. The daemon listens on every interface; on a machine with
// no LAN address it refuses invites (`not_on_lan`) and `invite` says so,
// for the test to skip.
import { type ChildProcessWithoutNullStreams, spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import type { Daemon } from "./fixtures";
import { ownerCookie } from "./questions-helpers";

/**
 * The environment of a CLI run as a person, on their scratch home, outside
 * any agent channel: no Clax setting but the home, and none of the
 * harnesses' variables (a harness session ID the test runner inherited would
 * make every shim one session).
 */
export function personEnv(home: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!/^(CLAX_|CLAUDE|CODEX_|GROK_|PI_CODING_AGENT)/.test(k)) env[k] = v;
  }
  env.CLAX_HOME = home;
  return env;
}

/** The owner's browser headers for a JSON call from the owner's own page. */
export function asOwner(d: Daemon) {
  return { cookie: ownerCookie(d.base, d.token), origin: d.base, "content-type": "application/json" };
}

/** A scratch git checkout `name` in `home`: an agent started there is named after it (spec §11). */
export function project(home: string, name: string) {
  const dir = join(home, "work", name);
  mkdirSync(join(dir, ".git"), { recursive: true });
  writeFileSync(join(dir, ".git", "HEAD"), "ref: refs/heads/main\n");
  return dir;
}

/** An invite command for `name`, or the reason the daemon cannot invite (no LAN address). */
export async function invite(d: Daemon, name: string): Promise<{ command: string } | { skip: string }> {
  const made = await fetch(`${d.base}/api/people/invites`, { method: "POST", headers: asOwner(d), body: JSON.stringify({ name }) });
  const v = await made.json();
  if (made.status === 409 && v.error?.code === "not_on_lan") return { skip: `no LAN address here: ${v.error?.message}` };
  if (made.status !== 201) throw new Error(`invite ${name}: ${made.status} ${JSON.stringify(v)}`);
  return { command: v.invite.command as string };
}

/** A connected person: their scratch home, their browser (made theirs by the browser link) and its page at the LAN origin. */
export type Person = { name: string; home: string; ctx: BrowserContext; page: Page; lan: string };

/**
 * `clax connect` with `command` from a new scratch home as `name`'s machine
 * (`--yes`: the owner's content reaches their agents when `yes`), then the
 * browser link in a new browser context.
 */
export async function connect(browser: Browser, command: string, name: string, yes = true): Promise<Person> {
  const home = mkdtempSync(join(tmpdir(), `clax-e2e-${name.toLowerCase()}-`));
  const args = ["connect", command.replace(/^clax connect /, ""), "--name", `${name.toLowerCase()}-laptop`, "--no-browser", ...(yes ? ["--yes"] : [])];
  const out = spawnSync(process.env.CLAX_E2E_BIN!, args, { env: personEnv(home), input: "n\n", encoding: "utf8", timeout: 20_000 });
  if (out.status !== 0) throw new Error(`clax connect as ${name}: ${out.status} ${out.stderr}`);
  const link = /http:\/\/\S+\/connect\/b\/\S+/.exec(out.stdout + out.stderr)?.[0];
  if (!link) throw new Error(`clax connect printed no browser link: ${out.stdout}${out.stderr}`);
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.goto(link);
  await page.waitForURL(/\/people$/);
  return { name, home, ctx, page, lan: new URL(link).origin };
}

/** Closes a person's browser and removes their home. */
export async function leave(p: Person | undefined) {
  if (!p) return;
  await p.ctx.close();
  rmSync(p.home, { recursive: true, force: true });
}

type Rpc = { result?: { content: { text: string }[]; isError?: boolean } };

/**
 * A person's agent: `clax mcp` on their home, started in `cwd`, driven over
 * JSON-RPC as Claude Code drives it. Every text any result carried is kept
 * in `seen` (feedback can arrive with any tool's result).
 */
export class Agent {
  seen = "";
  private next = 1;
  private waiting = new Map<number, (v: Rpc) => void>();
  private constructor(private p: ChildProcessWithoutNullStreams) {
    createInterface({ input: p.stdout }).on("line", l => {
      const m = JSON.parse(l) as { id?: number };
      if (m.id !== undefined) this.waiting.get(m.id)?.(m as Rpc);
    });
  }
  static async start(home: string, cwd: string): Promise<Agent> {
    // Under a shell of its own, as a harness runs its shim: the session is
    // matched by its parent process, so two shims spawned straight from
    // this test would join one session.
    const a = new Agent(spawn("/bin/sh", ["-c", '"$0" mcp; exit $?', process.env.CLAX_E2E_BIN!], { env: personEnv(home), cwd }));
    await a.rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "claude-code", version: "1" } });
    a.p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    return a;
  }
  private rpc(method: string, params: unknown) {
    const id = this.next++;
    const done = new Promise<Rpc>(r => this.waiting.set(id, r));
    this.p.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return done;
  }
  /** A tool's result object; an error result throws. */
  async call(name: string, args: unknown): Promise<Record<string, unknown>> {
    const r = await this.rpc("tools/call", { name, arguments: args });
    const texts = (r.result?.content ?? []).map(c => c.text ?? "");
    this.seen += `${texts.join("\n")}\n`;
    if (r.result?.isError) throw new Error(`${name}: ${texts[0]}`);
    return JSON.parse(texts[0] ?? "{}");
  }
  /** A tool's result object, or `{error}` with its text when it is an error result. */
  async attempt(name: string, args: unknown): Promise<Record<string, unknown>> {
    try { return await this.call(name, args); } catch (e) { return { error: String(e) }; }
  }
  /** Waits briefly for feedback; returns everything the agent has read so far. */
  async waitBriefly(): Promise<string> {
    await this.call("wait_for_feedback", { timeout_s: 1 });
    return this.seen;
  }
  stop() { this.p.stdin.end(); this.p.kill(); }
}

/** A new thread on `aid` v1, on its `h2`, written by the browser of `page` at `origin`. */
export async function threadFrom(page: Page, origin: string, aid: string, body: string): Promise<string> {
  const anchor = { kind: "element", selector: "body > main > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
  const r = await page.request.post(`${origin}/api/artifacts/${aid}/threads`, { headers: { origin }, multipart: { anchor: JSON.stringify(anchor), body, version: "1" } });
  if (r.status() !== 201) throw new Error(`thread: ${r.status()} ${await r.text()}`);
  return (await r.json()).thread.id as string;
}

/** A new thread on `aid` v1, on its `h2`, written by the owner's browser and sent to no one. */
export async function ownerThread(d: Daemon, aid: string, body: string): Promise<string> {
  const anchor = { kind: "element", selector: "body > main > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
  const form = new FormData();
  form.set("anchor", JSON.stringify(anchor));
  form.set("body", body);
  form.set("version", "1");
  const r = await fetch(`${d.base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers: { cookie: ownerCookie(d.base, d.token), origin: d.base } });
  if (r.status !== 201) throw new Error(`thread: ${r.status} ${await r.text()}`);
  return (await r.json()).thread.id as string;
}

/** Sends thread `tid` of `aid` to the agent `handle` from the browser of `page` at `origin`. */
export async function sendFrom(page: Page, origin: string, aid: string, tid: string, handle: string) {
  const r = await page.request.post(`${origin}/api/artifacts/${aid}/threads/${tid}/send`, { headers: { origin }, data: { to: handle } });
  if (r.status() !== 200) throw new Error(`send: ${r.status()} ${await r.text()}`);
}

/** The owner's view of every person, by display name (`GET /api/people`). */
export async function people(d: Daemon): Promise<Record<string, { public_id: string; agents: { handle: string; name: string }[] }>> {
  const v = await (await fetch(`${d.base}/api/people`, { headers: asOwner(d) })).json();
  return Object.fromEntries(v.people.map((p: { display_name: string }) => [p.display_name, p]));
}

/** The handle of `person`'s agent named `name`, as the owner's People lists it. */
export async function handleOf(d: Daemon, person: string, name: string): Promise<string> {
  const a = (await people(d))[person]?.agents.find(x => x.name === name);
  if (!a) throw new Error(`no agent ${name} of ${person}`);
  return a.handle;
}

/** The page of the artifact a thread is on, at `origin`, with that thread open. */
export const threadPage = (origin: string, aid: string, tid: string) => `${origin}/a/${aid}?thread=${tid}`;
