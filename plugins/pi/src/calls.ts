// Each Clax tool call's identity (spec 2026-10-06-toolpath-audit-design
// §6.7): a call ID minted when the call starts, the argument hash (§12.2)
// of the parameters Pi passed, Pi's own tool-call ID and tool name, and, for
// a tool that changes history, the session's git state (§9.3). Every request
// made for the call carries them in `x-clax-call` and `x-clax-git`; once the
// result has gone back to Pi, the call is reported in the background.
import { AsyncLocalStorage } from "node:async_hooks";
import { createHash, randomBytes } from "node:crypto";
import { encodeHeader, MAX_HEADER_BYTES, type GitField } from "./git.ts";

/** The tools whose calls capture git state: every tool that changes
 * history. The read-only tools, and `open`, which only shows a page,
 * capture none. */
export const GIT_TOOLS: ReadonlySet<string> = new Set([
  "publish",
  "comments_reply",
  "comments_resolve",
  "watch",
  "asset_upload",
  "db_set",
  "db_update",
  "db_delete",
  "db_str_replace",
  "db_batch",
  "delete",
  "pin",
  "unpin",
  "working",
  "ask",
]);

/** `v` in the JSON Canonicalization Scheme (RFC 8785): object keys sorted by
 * UTF-16 code units, no whitespace, numbers as ECMAScript writes them, and
 * strings escaped minimally (which is what `JSON.stringify` does). */
export function canonicalize(v: unknown): string {
  if (v === null) return "null";
  switch (typeof v) {
    case "boolean":
      return v ? "true" : "false";
    case "number":
      if (!Number.isFinite(v)) throw new TypeError("JCS has no form for a non-finite number");
      return JSON.stringify(v);
    case "string":
      return JSON.stringify(v);
    case "object": {
      if (Array.isArray(v)) return `[${v.map(canonicalize).join(",")}]`;
      const o = v as Record<string, unknown>;
      const keys = Object.keys(o).filter(k => o[k] !== undefined).sort();
      return `{${keys.map(k => `${JSON.stringify(k)}:${canonicalize(o[k])}`).join(",")}}`;
    }
    default:
      throw new TypeError(`JCS has no form for ${typeof v}`);
  }
}

/** `sha256:` and the lowercase hex SHA-256 of the canonical form of `args`
 * (absent arguments are `{}`). */
export function argsSha256(args: unknown): string {
  const canonical = canonicalize(args === undefined || args === null ? {} : args);
  return `sha256:${createHash("sha256").update(canonical, "utf8").digest("hex")}`;
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** A new ULID: 48 bits of milliseconds and 80 random bits, in Crockford's
 * base 32. */
export function newUlid(now = Date.now()): string {
  let time = "";
  let t = now;
  for (let i = 0; i < 10; i++) {
    time = CROCKFORD[t % 32] + time;
    t = Math.floor(t / 32);
  }
  const bytes = randomBytes(10);
  let bits = 0;
  let acc = 0;
  let rand = "";
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      rand += CROCKFORD[(acc >> bits) & 31];
    }
    acc &= (1 << bits) - 1;
  }
  return time + rand;
}

/** What `x-clax-call` carries. */
export interface CallHeader {
  call_id: string;
  tool: string;
  harness_tool?: string;
  args_sha256: string;
  started_at: string;
  harness_call_id?: string;
}

/** What the agent side posts once a call's result has gone back. */
export interface ToolCallReport extends CallHeader {
  ended_at: string;
  outcome: "ok" | "error";
  artifact_id?: string;
}

/** One tool call in progress. */
export class CallScope {
  /** The `x-clax-call` value; undefined when the identity does not fit a
   * header. */
  readonly encoded: string | undefined;
  private gitDone: GitField | undefined;
  private artifact: string | undefined;

  constructor(readonly header: CallHeader, private readonly git?: Promise<GitField>) {
    const h = Buffer.from(JSON.stringify(header), "utf8").toString("base64url");
    this.encoded = h.length <= MAX_HEADER_BYTES ? h : undefined;
    git?.then(f => { this.gitDone = f; }, () => { this.gitDone = { capture: "unavailable" }; });
  }

  /** Whether the call carries git state. */
  hasGit(): boolean {
    return this.git !== undefined;
  }

  /** The `x-clax-git` value, waiting for the capture (it ends by its own
   * deadline). */
  async gitHeader(): Promise<string | undefined> {
    if (!this.git) return undefined;
    const f = await this.git.catch((): GitField => ({ capture: "unavailable" }));
    return encodeHeader(f);
  }

  /** The `x-clax-git` value if the capture has finished. */
  gitHeaderNow(): string | undefined {
    return this.gitDone ? encodeHeader(this.gitDone) : undefined;
  }

  /** Notes the artifact the call's arguments named; the first is kept. */
  noteArtifact(id: string): void {
    this.artifact ??= id;
  }

  /** The report of this call, ending now with `outcome`. */
  report(outcome: "ok" | "error"): ToolCallReport {
    const now = new Date().toISOString();
    const r: ToolCallReport = { ...this.header, ended_at: now < this.header.started_at ? this.header.started_at : now, outcome };
    if (this.artifact !== undefined) r.artifact_id = this.artifact;
    return r;
  }
}

/** Starts a call of Clax tool `tool`, which Pi named `harnessTool` and
 * gave the ID `harnessCallId`, with `params`; `git`, for a tool in
 * [`GIT_TOOLS`], starts its capture. Undefined when `params` has no
 * canonical form (JCS has none for a non-finite number, say): the call then
 * runs unrecorded, since the hash it would carry is required. */
export function beginCall(opts: {
  tool: string;
  harnessTool: string;
  harnessCallId: string | undefined;
  params: unknown;
  git?: () => Promise<GitField>;
}): CallScope | undefined {
  let args_sha256: string;
  try {
    args_sha256 = argsSha256(opts.params);
  } catch {
    return undefined;
  }
  const header: CallHeader = {
    call_id: newUlid(),
    tool: opts.tool,
    harness_tool: opts.harnessTool,
    args_sha256,
    started_at: new Date().toISOString(),
  };
  if (opts.harnessCallId) header.harness_call_id = opts.harnessCallId;
  return new CallScope(header, GIT_TOOLS.has(opts.tool) && opts.git ? opts.git() : undefined);
}

const current = new AsyncLocalStorage<CallScope>();

/** Runs `f` as part of call `scope`: every daemon request it makes carries
 * the call. */
export function inCall<T>(scope: CallScope, f: () => Promise<T>): Promise<T> {
  return current.run(scope, f);
}

/** The call the current code runs for, if any. */
export function currentCall(): CallScope | undefined {
  return current.getStore();
}
