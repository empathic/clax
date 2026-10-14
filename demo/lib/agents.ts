// Scripted agents that write one artifact together (the demo of several
// agents working at once), each its own MCP session under one agent token.
// Each agent owns one section of a product brief (lib/brief.ts). In every
// round all of them read the same version and then publish, one after
// another in an order that rotates each round, so all but the first get
// `version_conflict`; each loser reads the version that won, merges its own
// section into it and publishes again. While it works an agent says what it
// is doing with `working`, and with `draft` shows people its section
// growing (a Clax whose tool set has no `draft` is logged once and skipped). After drafting, every agent waits for comments
// sent to it (`wait_for_feedback`) and answers each with a new version, a
// reply and a resolve; a comment held from it (by a person its own has not
// approved) is logged as the placeholder it is.
//
// Everything goes through the MCP endpoint, so it is safe to point at a
// deployed instance: it creates one artifact (or writes to the one
// `artifactId` names) and touches nothing else. No model is called.

import { BRIEF_TITLE, replaceSection, revisedBody, sectionHtml, SECTIONS, skeleton, type SectionPlan } from "./brief.ts";
import { ClaxApiError, McpAgent, NATIVE_MCP_PATH, toolError } from "./http.ts";


/** The least time between one agent's drafts, in ms: a session may draft
 * twice a second (DRAFT_RATE in crates/clax-core/src/drafts.rs). */
const DRAFT_GAP_MS = 600;

export interface DemoOptions {
  url: string;
  token: string;
  /** The MCP endpoint's path: `/mcp/json` on the native daemon (the default), `/mcp` on the Worker. */
  mcpPath?: string;
  agents: number;
  /**
   * Milliseconds the first agent of a round spends "working" before it
   * publishes; each later one takes a third of this longer. 0 for tests
   * (all publish at once), a few seconds for watching.
   */
  paceMs: number;
  /** Seconds to keep waiting for comments after the drafts; 0 skips the feedback phase. */
  listenS: number;
  /** Stop listening after this many comments are answered, across all agents. */
  stopAfter?: number;
  /** Write into this artifact instead of creating one; it must have the brief's section markers. */
  artifactId?: string;
  log?: (line: string) => void;
  /** Called with the artifact and the agents' names once it exists, before the agents start drafting. */
  onArtifact?: (a: { id: string; url: string; agents: string[] }) => void | Promise<void>;
  /** Called once every agent is waiting for comments. */
  onListening?: () => void | Promise<void>;
  /** Aborts the run. */
  signal?: AbortSignal;
}

export interface DemoResult {
  id: string;
  url: string;
  /** Versions the agents published, in the order they landed. */
  versions: { n: number; agent: string; note: string }[];
  /** Conflicts met and merged: the agent and the version that was current. */
  conflicts: { agent: string; at: number }[];
  /** Comments answered: thread, agent and the version that answered it. */
  answered: { thread_id: string; agent: string; version: number }[];
  /** Comments held from an agent: thread, agent and the authors withheld. */
  held: { thread_id: string; agent: string; authors: string[] }[];
  /** Drafts people were shown: the agent and how many words of its section it had written. */
  drafts: { agent: string; words: number }[];
  agents: string[];
}

interface Head {
  n: number;
  html: string;
}

const delay = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve) => {
    if (ms <= 0 || signal?.aborted) return resolve();
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => { clearTimeout(t); resolve(); }, { once: true });
  });

/** The current version of `id` and the text of its index.html, as `agent` reads it. */
async function readHead(agent: McpAgent, id: string): Promise<Head> {
  const r = await agent.ok("read", { url_or_id: id });
  const html = r.data.content;
  if (typeof html !== "string") throw new Error(`${agent.name} cannot read index.html of ${id}`);
  return { n: r.data.version as number, html };
}

/** The ID of the thread a feedback or withheld entry is on. */
function threadOf(f: Record<string, unknown>): string {
  const t = f.thread as Record<string, unknown> | undefined;
  return String(f.thread_id ?? t?.id ?? "");
}

/** The artifact a feedback or withheld entry is on. */
function artifactOf(f: Record<string, unknown>): string {
  const a = f.artifact as Record<string, unknown> | undefined;
  return String(f.artifact_id ?? a?.id ?? "");
}

/** The newest comment of a feedback entry: its author's name and its text. */
function commentOf(f: Record<string, unknown>): { author: string; body: string } {
  const comments = (f.comments ?? (f.thread as Record<string, unknown> | undefined)?.comments ?? []) as Record<string, unknown>[];
  const last = comments[comments.length - 1] ?? f;
  const who = last.author as Record<string, unknown> | string | undefined;
  const author = typeof who === "string" ? who : String(who?.name ?? who?.display_name ?? last.author_name ?? "someone");
  return { author, body: String(last.body ?? last.text ?? "") };
}

export async function runDemo(o: DemoOptions): Promise<DemoResult> {
  const log = o.log ?? (() => undefined);
  if (!Number.isInteger(o.agents) || o.agents < 1 || o.agents > SECTIONS.length) {
    throw new Error(`--agents must be from 1 to ${SECTIONS.length}`);
  }
  const plans = SECTIONS.slice(0, o.agents);
  const result: DemoResult = { id: "", url: "", versions: [], conflicts: [], answered: [], held: [], agents: [], drafts: [] };
  // Each agent is its own MCP session under the one token, named after its client.
  const agents = await Promise.all(plans.map((p) => new McpAgent(o.url, o.mcpPath ?? NATIVE_MCP_PATH, o.token, `${p.key}-writer`).connect()));
  result.agents = agents.map((a) => a.name);
  log(`connected ${result.agents.join(", ")}`);
  const latest = new Map<string, string>(plans.map((p) => [p.key, ""]));
  // Whether the tool set has `draft` (live drafts); found out on the first try.
  let drafts: boolean | null = null;
  // When each agent last drafted (ms since the epoch), to keep to the draft rate.
  const lastDraft = new Map<string, number>();

  /**
   * Shows people `agent`'s section growing toward `body` over `base`: up
   * to `wanted` drafts, each with more of the body's words, spread over
   * `ms`. Without `draft`, it only takes the time. One agent's drafts are
   * at least DRAFT_GAP_MS apart (fewer of them when `ms` is short, and a
   * wait when the last was too recent), within a session's draft rate. A
   * draft refused is logged and skipped: drafts are a view of the work,
   * never part of it.
   */
  async function draftSection(agent: McpAgent, plan: SectionPlan, body: string, base: Head, wanted: number, ms: number, note: string): Promise<void> {
    const steps = Math.max(1, Math.min(wanted, Math.floor(ms / DRAFT_GAP_MS)));
    const tokens = body.match(/<[^>]*>|[^<\s]+\s*|\s+/g) ?? [body];
    const words = tokens.filter((t) => !t.startsWith("<") && t.trim() !== "").length;
    let over = base;
    for (let k = 1; k <= steps; k++) {
      await delay(ms / steps, o.signal);
      if (o.signal?.aborted || drafts === false) continue;
      const since = Date.now() - (lastDraft.get(agent.name) ?? 0);
      if (since < DRAFT_GAP_MS) await delay(DRAFT_GAP_MS - since, o.signal);
      if (o.signal?.aborted) continue;
      lastDraft.set(agent.name, Date.now());
      const upto = Math.ceil((tokens.length * k) / steps);
      const section = sectionHtml(plan, agent.name, tokens.slice(0, upto).join(""));
      // A draft is of the version it changes: once another agent's version
      // lands, the agent drafts over that one instead.
      let a;
      let err;
      for (let attempt = 0; attempt < 3; attempt++) {
        const html = replaceSection(over.html, plan.key, section);
        if (html === null) return;
        try {
          a = await agent.call("draft", { url_or_id: result.id, html, if_version: over.n, note });
        } catch (e) {
          if (e instanceof ClaxApiError && /tool not found/.test(e.message)) {
            if (drafts === null) log("this Clax has no draft tool: the agents publish without live drafts");
            drafts = false;
            return;
          }
          throw e;
        }
        drafts = true;
        err = toolError(a);
        if (err === null || (err.code !== "conflict" && err.code !== "version_conflict")) break;
        over = await readHead(agent, result.id);
      }
      if (err !== null && err !== undefined) {
        log(`${agent.name}: draft not shown (${err.code}: ${err.message})`);
        continue;
      }
      const shown = tokens.slice(0, upto).filter((t) => !t.startsWith("<") && t.trim() !== "").length;
      result.drafts.push({ agent: agent.name, words: shown });
      log(`${agent.name}: drafting ${plan.key} (${shown}/${words} words)`);
    }
  }

  /**
   * Publishes `agent`'s section into `base`, merging into the current
   * version on every conflict; null, publishing nothing, once the run is
   * stopped.
   */
  async function publishSection(agent: McpAgent, plan: SectionPlan, body: string, base: Head, note: string, addresses?: string[]): Promise<number | null> {
    let head = base;
    for (let attempt = 0; attempt < 20; attempt++) {
      if (o.signal?.aborted) return null;
      const html = replaceSection(head.html, plan.key, sectionHtml(plan, agent.name, body));
      if (html === null) throw new Error(`version ${head.n} of ${result.id} has no ${plan.key} section`);
      // A version that took a merge says so in its note, which the version strip shows.
      const said = attempt === 0 ? note : `merged onto v${head.n} after a conflict · ${note}`;
      const a = await agent.call("publish", { id: result.id, html, if_version: head.n, note: said, ...(addresses ? { addresses } : {}) });
      const e = toolError(a);
      if (e === null) {
        const n = a.data.version as number;
        result.versions.push({ n, agent: agent.name, note: said });
        log(`${agent.name}: published v${n} (${said})`);
        return n;
      }
      // Clax answers a stale `if_version` with `conflict`.
      if (e.code !== "conflict" && e.code !== "version_conflict") throw new ClaxApiError(0, e.code, e.message);
      const current = (e.current_version as { n?: number } | number | undefined);
      const at = typeof current === "number" ? current : current?.n ?? 0;
      result.conflicts.push({ agent: agent.name, at });
      log(`${agent.name}: conflict, v${at} landed first; merging`);
      head = await readHead(agent, result.id);
    }
    throw new Error(`${agent.name} could not publish after 20 conflicts`);
  }

  try {
    if (o.artifactId === undefined) {
      const created = await agents[0]!.ok("publish", {
        title: BRIEF_TITLE,
        html: skeleton(plans),
        note: `Outline: ${plans.length} sections, one per agent`,
      });
      result.id = created.data.artifact_id as string;
      result.url = created.data.url as string;
      result.versions.push({ n: 1, agent: agents[0]!.name, note: "outline" });
      log(`${agents[0]!.name}: created ${result.url}`);
    } else {
      result.id = o.artifactId;
      result.url = `${o.url.replace(/\/+$/, "")}/a/${o.artifactId}`;
      await readHead(agents[0]!, o.artifactId);
    }
    // Watching it, each agent is one a person may send a thread to.
    await Promise.all(agents.map((a) => a.ok("watch", { url_or_id: result.id })));
    await o.onArtifact?.({ id: result.id, url: result.url, agents: result.agents });

    const rounds = Math.min(...plans.map((p) => p.drafts.length));
    for (let r = 0; r < rounds && o.signal?.aborted !== true; r++) {
      await Promise.all(agents.map((a, i) => a.ok("working", { url_or_id: result.id, message: plans[i]!.working[r]! })));
      // Every agent reads the same version, so every publish but the first
      // conflicts and merges. Each agent works its own while before it
      // publishes, a third of the pace longer per place, and the places
      // rotate each round, so a different agent lands first each time.
      const bases = await Promise.all(agents.map((a) => readHead(a, result.id)));
      await Promise.all(
        agents.map(async (a, i) => {
          const place = (i - r + agents.length * rounds) % agents.length;
          const plan = plans[i]!;
          const body = plan.drafts[r]!;
          await draftSection(a, plan, body, bases[i]!, o.paceMs > 0 ? 3 : 1, o.paceMs + Math.round((o.paceMs / 3) * place), plan.working[r]!);
          latest.set(plan.key, body);
          await publishSection(a, plan, body, bases[i]!, `${plan.heading}: draft ${r + 1}`);
        }),
      );
    }

    if (o.listenS > 0 && o.signal?.aborted !== true) await listen();
    return result;
  } finally {
    await Promise.all(agents.map(async (a) => {
      if (result.id !== "") await a.call("working", { url_or_id: result.id, done: true }).catch(() => undefined);
      await a.close();
    }));
  }

  async function listen(): Promise<void> {
    const deadline = Date.now() + o.listenS * 1000;
    // Ends every wait in flight once listening is over (Ctrl-C, or enough
    // comments answered). A wait abandoned this way may already have taken
    // feedback, which the server then counts as delivered.
    const stop = new AbortController();
    o.signal?.addEventListener("abort", () => stop.abort(), { once: true });
    log(`agents are waiting for comments for ${o.listenS} s: comment on ${result.url} and send the thread to one of ${result.agents.join(", ")}`);
    const done = () => stop.signal.aborted || Date.now() >= deadline || (o.stopAfter !== undefined && result.answered.length >= o.stopAfter);
    const told = new Set<string>();
    let waiting = 0;
    await Promise.all(
      agents.map(async (agent, i) => {
        const plan = plans[i]!;
        let first = true;
        while (!done()) {
          await agent.ok("working", { url_or_id: result.id, message: `waiting for comments on ${plan.heading.toLowerCase()}` });
          const left = Math.ceil((deadline - Date.now()) / 1000);
          if (left < 1) break;
          if (first) {
            first = false;
            if (++waiting === agents.length) await o.onListening?.();
          }
          let w;
          try {
            w = await agent.ok("wait_for_feedback", { url_or_id: result.id, timeout_s: Math.min(20, left) }, stop.signal);
          } catch (e) {
            if (stop.signal.aborted) break;
            throw e;
          }
          for (const h of (w.data.withheld ?? []) as Record<string, unknown>[]) {
            const tid = threadOf(h);
            const authors = typeof h.author === "string" ? [h.author] : ((h.authors ?? []) as { name?: string }[]).map((a) => a.name ?? "someone");
            if (told.has(`${agent.name} ${tid}`)) continue;
            told.add(`${agent.name} ${tid}`);
            result.held.push({ thread_id: tid, agent: agent.name, authors });
            log(`${agent.name}: a comment from ${authors.join(", ") || "someone"} on thread ${tid} is withheld until its person approves them${h.asked === true ? " (its person was asked)" : ""}`);
          }
          for (const f of (w.data.feedback ?? []) as Record<string, unknown>[]) {
            if (artifactOf(f) !== "" && artifactOf(f) !== result.id) continue;
            const tid = threadOf(f);
            const { author, body: comment } = commentOf(f);
            log(`${agent.name}: comment from ${author}: ${JSON.stringify(comment)}`);
            await agent.ok("working", { url_or_id: result.id, message: `revising ${plan.heading.toLowerCase()} for ${author}`, thread_ids: [tid] });
            const body = revisedBody(latest.get(plan.key)!, author, comment);
            latest.set(plan.key, body);
            const base = await readHead(agent, result.id);
            await draftSection(agent, plan, body, base, o.paceMs > 0 ? 2 : 1, o.paceMs, `revising for ${author}`);
            const n = await publishSection(agent, plan, body, base, `${plan.heading}: revised for ${author}`, [tid]);
            if (n === null) break;
            await agent.ok("comments_reply", { url_or_id: result.id, thread_id: tid, text: `Revised the ${plan.heading} section in version ${n}.` });
            await agent.ok("comments_resolve", { url_or_id: result.id, thread_id: tid });
            result.answered.push({ thread_id: tid, agent: agent.name, version: n });
            log(`${agent.name}: replied on ${tid} and resolved it (v${n})`);
            if (o.stopAfter !== undefined && result.answered.length >= o.stopAfter) stop.abort();
          }
        }
      }),
    );
  }
}
