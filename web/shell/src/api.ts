import type { PresenceView } from "./view/presence-model";
import type { Working } from "./view/working-model";

export type Artifact = {
  id: string; title: string; description: string | null; icon: string | null;
  created_at?: string; updated_at: string; current_version: number; pinned: boolean;
  capabilities?: Record<string, unknown>; contract_version?: string; owner_session_id?: string | null;
  /** From `GET /api/artifacts` and `GET /api/artifacts/<id>`: the owner session exists and has not ended. */
  owner_live?: boolean;
  /** From `GET /api/artifacts` and `GET /api/artifacts/<id>`: the owner session's harness, when it exists. */
  owner_harness?: string | null;
  /** From `GET /api/artifacts` and `GET /api/artifacts/<id>`: the artifact's people and agents. */
  participants?: Participants;
  /** Who is working on it now (never a session ID). */
  working?: Working[];
  /** `html` (published by an agent) or `live` (a live page, made from Chrome with the Clax extension). */
  kind?: "html" | "live";
  /** A live page's key: its origin (its site's key), its path, the page on
   * its site's most recently used origin, and its site's origins, the most
   * recently used first (spec 2026-10-05 §7.2). */
  live?: { origin: string; path: string; page_url: string; origins?: string[]; merged_into?: string; merged_into_url?: string | null } | null;
};
/** `agents` is ordered live first, then most recently active; `live` means a send can reach it. */
export type Participants = { people: { public_id: string; display_name: string | null; seen: number | null }[]; agents: { handle: string; harness: string; live: boolean }[] };
export type AttentionSummary = { addressed: string[]; addressed_v: number | null; new_replies: string[]; open_in: string[]; seen: number | null };
export type Attention = AttentionSummary & { looked: Record<string, string> };
/** `sha256`: the file's lowercase hex SHA-256; absent on versions written before content hashes. */
export type FileMeta = { content_type: string; size: number; sha256?: string };
export type Version = {
  artifact_id: string; n: number; label: string | null; created_at: string; files: Record<string, FileMeta>;
  /** The publishing session's agent handle and harness. */
  agent?: string | null; agent_harness?: string | null;
  /** `sha256:` and the hex SHA-256 of the file manifest; `null` on versions written before content hashes. */
  content_sha256?: string | null;
  /** The agent's note on what changed; the IDs of the threads this version addresses. */
  note?: string | null; addresses?: string[];
};

/** A non-OK API response; `status` is the HTTP status code, `code` the
 * daemon's error code when the response named one. */
export class ApiError extends Error {
  constructor(public status: number, message: string, public code: string | null = null) {
    super(`${status} ${message}`);
    this.name = "ApiError";
  }
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not json */ }
    throw new ApiError(res.status, msg);
  }
  return res.json() as Promise<T>;
}

/** Every live artifact; with `only`, that artifact alone, or none when it is not live. */
export async function listArtifacts(only?: string, init?: RequestInit): Promise<Artifact[]> {
  return (await json<{ artifacts: Artifact[] }>(await (init ? fetch(only ? `/api/artifacts?artifact=${only}` : "/api/artifacts", init) : fetch(only ? `/api/artifacts?artifact=${only}` : "/api/artifacts")))).artifacts;
}
/** With the viewer cookie, the answer also carries the viewer's `attention`. */
export async function getArtifact(id: string, init?: RequestInit): Promise<{ artifact: Artifact; versions: Version[]; attention?: Attention }> {
  return json(await (init ? fetch(`/api/artifacts/${id}`, init) : fetch(`/api/artifacts/${id}`)));
}

/** This viewer's attention on every live artifact (with `only`, on that one
 * artifact, or none when it is not live); {} without a viewer, null on failure. */
export async function getAttention(only?: string): Promise<Record<string, AttentionSummary> | null> {
  try { const r = await fetch(`/api/viewers/me/attention${only ? `?artifact=${only}` : ""}`); return r.ok ? (await r.json()).artifacts : null; } catch { return null; }
}
/** Records that this viewer looked at `ids`; answers the viewer's marks on `aid`, or null on failure (the next look writes again). */
export async function putLooked(aid: string, ids: string[]): Promise<Record<string, string> | null> {
  try {
    const r = await fetch("/api/viewers/me/looked", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, thread_ids: ids }) });
    return r.ok ? (await r.json()).looked : null;
  } catch { return null; }
}

/** Raises this viewer's version seen mark on `aid` to `n`; a failure is ignored (the next load writes again). */
export async function putSeen(aid: string, n: number): Promise<void> {
  try {
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, version: n }) });
  } catch { /* the next load writes again */ }
}

/** Reports this viewer here or away on `aid` from tab `tab`, with where they look; answers
 * the artifact's presence, or null on failure (the next report writes again). */
export async function putPresence(aid: string, state: "here" | "away", where: string | null, tab: string, keepalive = false): Promise<PresenceView[] | null> {
  try {
    const r = await fetch("/api/viewers/me/presence", { method: "PUT", keepalive, headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, state, tab, ...where && { where } }) });
    const people = r.ok ? (await r.json()).people : null;
    return Array.isArray(people) ? people : null;
  } catch { return null; }
}
/** The artifact's presence, or null on failure. */
export async function getPresence(aid: string): Promise<PresenceView[] | null> {
  try {
    const r = await fetch(`/api/artifacts/${aid}/presence`);
    const people = r.ok ? (await r.json()).people : null;
    return Array.isArray(people) ? people : null;
  } catch { return null; }
}

let tokenPromise: Promise<string | null> | null = null;
const tokenHeard = new Set<() => void>();
let tokenFailed = false;
/** Calls `fn` each time a token request answers with the token after an
 * earlier one failed: only then did this page gain the events cookie. */
export const onToken = (fn: () => void) => { tokenHeard.add(fn); };
/** The write token, only served to loopback browsers; null on a LAN viewer. */
export function getToken(): Promise<string | null> {
  if (!tokenPromise) {
    const p: Promise<string | null> = fetch("/api/token").then(async r => {
      if (r.ok) {
        const t = (await r.json()).token as string;
        if (tokenFailed) { tokenFailed = false; for (const f of tokenHeard) f(); }
        return t;
      }
      if (r.status === 403) return null;
      throw new Error(`token request failed: ${r.status}`);
    }).catch(() => {
      tokenFailed = true;
      if (tokenPromise === p) tokenPromise = null;
      return null;
    });
    tokenPromise = p;
  }
  return tokenPromise;
}
export async function patchArtifact(id: string, patch: Partial<Pick<Artifact, "title" | "description" | "icon" | "pinned">>, token: string): Promise<Artifact> {
  return (await json<{ artifact: Artifact }>(await fetch(`/api/artifacts/${id}`, { method: "PATCH", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(patch) }))).artifact;
}
export async function deleteArtifact(id: string, token: string): Promise<void> {
  await json<unknown>(await fetch(`/api/artifacts/${id}`, { method: "DELETE", headers: { authorization: `Bearer ${token}` } }).then(r => (r.status === 204 ? new Response("{}") : r)));
}

export type SampleLimits = { maxPromptBytes: number; images?: { maxCount: number; maxInputBytes: number; mediaTypes: string[] }; tools?: { maxCount: number } };
/** `GET /api/artifacts/<id>/sample` with the token: whether this daemon samples for the artifact, and today's count. */
export type SampleStatus = { available: boolean; provider: string | null; limits: SampleLimits; calls_today: number; daily_call_cap: number | null };

/** The artifact's sample status as the owner's browser sees it, or null when it cannot be read. */
export async function getSampleStatus(id: string, token: string): Promise<SampleStatus | null> {
  try {
    const res = await fetch(`/api/artifacts/${id}/sample`, { headers: { authorization: `Bearer ${token}` } });
    return res.ok ? ((await res.json()) as SampleStatus) : null;
  } catch {
    return null;
  }
}

// Agent questions and the owner's inbox (spec
// 2026-10-06-agent-questions-and-inbox §5, §7.6, §8; docs/contract.md "Agent
// questions", "The inbox"). Owner only. Every string in these shapes is
// agent text or derived from it: untrusted, rendered as text. Their
// fetchers live with the lazily loaded question module (`q/api.ts`).

/** One option of a choice question. */
export type QOption = { label: string; description?: string; preview?: string; recommended?: boolean };
/** One question: `options` is empty for a free-text question. */
export type QuestionSpec = { question: string; header: string; options: QOption[]; multi_select: boolean; other: boolean };
/** The person's answer to one question: one or more labels and/or the "Other" text. */
export type QAnswer = { selected: string[]; text: string | null };
/** The answer body: one entry per question, in order. */
export type AnswerBody = { answers: QAnswer[] };
/** The asking agent; `project` is the last component of its working directory. */
export type QAgent = { handle: string; harness: string; project: string };
export type QuestionStatus = "open" | "answered" | "declined" | "released" | "withdrawn";
/** The question view, as every owner route and `question` event gives it. */
export type QuestionView = {
  id: string; agent: QAgent | null;
  /** The artifact or live page it is about; null for none, or when deleted. */
  artifact: { id: string; title: string; kind: string } | null;
  /** `hook` is a mirrored `AskUserQuestion`. */
  source: "ask" | "hook"; status: QuestionStatus;
  questions: QuestionSpec[]; answers: QAnswer[] | null;
  answered_via: "shell" | "extension" | "cli" | "terminal" | null;
  created_at: string; closed_at: string | null;
};

export type InboxKind = "reply" | "version" | "published" | "question" | "finished" | "approval" | "connection" | "ended";
/** A `connection` item's grant: a request waiting for the owner, or a
 * machine that connected with an invite (`person` once it has one). */
export type InboxConnection = {
  id: string; status: string; via: "invite" | "request";
  client: { name: string; key: string | null; peer_ip: string };
  person: { public_id: string; display_name: string | null } | null;
};
/** A person as an approval card names them: `owner` when it is the
 * owner, who is named "The owner" until they choose a name. */
export type CardPerson = { public_id: string | null; name: string; owner?: boolean };
/** A comment an approval would let through: `by_agent` when an agent of
 * its author wrote it. */
export type HeldComment = { id: string; body: string; created_at: string; author_name: string | null; by_agent: boolean };
/** What an approval request holds: its thread's comments by the author, or,
 * outside threads, what the author holds at the page (its title and
 * description, files, data paths and uploads). */
export type HeldContent =
  | { thread: { id: string; status: string } | null; comments: HeldComment[] }
  | { title: string | null; description: string | null; files: string[]; docs: string[]; assets: { id: string; content_type: string; size: number }[] };
/** An `approval` item's card (spec 2026-10-08-multi-agent-multi-person
 * §9.5): the request (its author, the agent it is for, its page and
 * thread, how much waits, its state and the scope it was approved at),
 * the withheld content itself, and, for a page's send, the page's authors
 * and the viewer whose browser ran it. */
export type InboxApproval = {
  id: string; approver: string | null; author: CardPerson; artifact_id: string; thread_id: string | null;
  agent: { handle: string | null; name: string; harness: string; live: boolean } | null;
  cause: string; count: number; status: "open" | "approved" | "declined" | "withdrawn";
  scope: "thread" | "artifact" | "everywhere" | null;
  created_at: string; updated_at?: string; decided_at: string | null;
  content: HeldContent;
  via_page: { authors: CardPerson[]; viewer: CardPerson } | null;
};
/** A thread an item names; `summary` and `status` are null when it is gone. */
export type InboxThread = { id: string; summary: string | null; status?: string | null };
/** The inbox item view. Each kind fills its own field (`reply` also fills
 * `thread`); a source's text fields are null when `gone`. `seq` orders items
 * (higher is newer) and is the cursor and a bulk mark's `upto`. */
export type InboxItem = {
  id: string; seq: number; kind: InboxKind; read: boolean; created_at: string;
  agent: QAgent | null;
  /** `title` and `kind` are null when the artifact is deleted. */
  artifact: { id: string; title: string | null; kind: string | null; page_url: string | null } | null;
  thread: InboxThread | null;
  reply: { comment_id: string; body: string | null; addressed: boolean } | null;
  version: { n: number; note: string | null; addressed: InboxThread[] } | null;
  published: { description: string | null } | null;
  question: QuestionView | null;
  work: { message: string | null; threads: InboxThread[] } | null;
  /** `connection`, `approval` and `ended` items only. */
  /** `connection`: null once the request is gone. */
  connection?: InboxConnection | null;
  /** `approval`: its card, or its ID alone once the request is gone. */
  approval?: InboxApproval | { id: string };
  /** `ended`: what the agent's end cleaned up; `text` is the note's line
   * ("clax-claude ended · released 2 threads · withdrew 1 question"). */
  ended?: InboxEnded | null;
  gone: boolean;
  /** Where opening the item leads (a path on the daemon). */
  url: string;
};
/** An `ended` item's note (spec 2026-10-08 §14, M25). */
export type InboxEnded = {
  name: string | null;
  reason: "exit" | "idle" | "revoked" | "person";
  text: string;
  questions: number;
  requests: number;
  /** The open threads it released; `title` is null once the page is gone. */
  threads: { id: string; artifact_id: string; title: string | null; url: string }[];
};
/** One page of `GET /api/inbox`; `total` is present when the query filters. */
export type InboxPage = { items: InboxItem[]; next_cursor: string | null; unread: number; total?: number | "10000+" };
/** `GET /api/inbox/summary`: open questions oldest first, the five newest unread other items. */
export type InboxSummary = { unread: number; questions: QuestionView[]; latest: InboxItem[] };
/** The inbox page's search: text (`q`), kinds, an artifact ID, an agent
 * (harness or handle), dates (`YYYY-MM-DD` or RFC 3339), and read state. */
export type InboxFilter = { q?: string; kind?: InboxKind[]; artifact?: string; agent?: string; since?: string; until?: string; read?: "unread" | "read" | "all" };

// People (spec 2026-10-08-multi-agent-multi-person §6, §7.4, §9, §17). The
// owner's and a connected person's People page reads these; its fetchers
// load with the page (`people/api.ts`). Names are people's and agents'
// text: untrusted, rendered as text.

/** A GNAP access object (§7.1): `locations` are `*`, `own`, artifact IDs
 * (and, in computed rights, `own:<id>`); `level` is the db level. */
export type Access = { type: string; actions: string[]; locations?: string[]; level?: string };
/** A rights array: what any one access object allows. */
export type Rights = Access[];
/** A connected machine (a grant), as the owner and its person see it. */
export type PeopleGrant = {
  id: string; status: string; via: string;
  /** `key` is the key's fingerprint (`SHA256:…`). */
  client: { name: string; key: string | null; peer_ip: string };
  person: { public_id: string; display_name: string | null } | null;
  /** A machine of the owner's. */
  owner: boolean;
  narrow: Rights | null;
  created_at: string; decided_at: string | null; last_used_at: string | null; ended_at: string | null; ended_by: string | null;
};
/** A live agent with its narrowing, elevation and effective rights;
 * `grant_id` and `rights` are null for an agent of the daemon's own
 * machine, which rights do not limit. */
export type PeopleAgent = {
  handle: string; name: string; harness: string; person: string | null; grant_id: string | null; started_at: string;
  narrow: Rights | null; elevate: Rights | null; rights: Rights | null;
};
/** A person as the owner's People lists them; `revoked_at` once revoked. */
export type PeoplePerson = {
  public_id: string; display_name: string | null; revoked_at: string | null;
  rights: Rights; grants: PeopleGrant[]; agents: PeopleAgent[];
};
/** `GET /api/people/me`: the caller, their machines and their live agents. */
export type PeopleMe = {
  person: { public_id: string | null; display_name: string | null; owner: boolean; rights: Rights };
  grants: PeopleGrant[]; agents: PeopleAgent[];
};
/** `GET /api/people`: everyone, and the `*unconnected` switch (§9.6). */
export type PeopleList = { people: PeoplePerson[]; unconnected: { on: boolean; approval_id: string | null } };
export type PeopleInvite = {
  id: string; person: { public_id: string | null; display_name: string | null }; owner: boolean;
  created_at: string; expires_at: string; used_at: string | null; grant_id: string | null; revoked_at: string | null;
};
/** A new invite: `command` carries its code, shown this once. */
export type NewInvite = { id: string; command: string; expires_at: string; person: { public_id: string | null; display_name: string | null } };
/** An approval: `approver`'s agents may see `author`'s content at `scope`;
 * `author.owner` when the author is the owner; `artifact_title` is its
 * page's title while the page is live. */
export type PeopleApproval = {
  id: string; approver: string | null; author: { public_id: string | null; name: string; owner?: boolean };
  scope: "thread" | "artifact" | "everywhere"; artifact_id: string | null; artifact_title?: string | null; thread_id: string | null;
  created_at: string; created_via: string; request_id: string | null;
};
/** An approval request, never with the withheld content. */
export type PeopleRequest = {
  id: string; author: { public_id: string; name: string; owner?: boolean }; artifact_id: string; thread_id: string | null;
  agent: { handle: string | null; name: string; harness: string; live: boolean } | null;
  cause: string; count: number; status: string; created_at: string;
};
