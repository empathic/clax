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
export type FileMeta = { content_type: string; size: number };
export type Version = {
  artifact_id: string; n: number; label: string | null; created_at: string; files: Record<string, FileMeta>;
  /** The publishing session's agent handle and harness. */
  agent?: string | null; agent_harness?: string | null;
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
/** The write token, only served to loopback browsers; null on a LAN viewer. */
export function getToken(): Promise<string | null> {
  if (!tokenPromise) {
    const p: Promise<string | null> = fetch("/api/token").then(async r => {
      if (r.ok) return (await r.json()).token as string;
      if (r.status === 403) return null;
      throw new Error(`token request failed: ${r.status}`);
    }).catch(() => {
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
