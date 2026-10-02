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

export async function listArtifacts(): Promise<Artifact[]> {
  return (await json<{ artifacts: Artifact[] }>(await fetch("/api/artifacts"))).artifacts;
}
/** With the viewer cookie, the answer also carries the viewer's `attention`. */
export async function getArtifact(id: string): Promise<{ artifact: Artifact; versions: Version[]; attention?: Attention }> {
  return json(await fetch(`/api/artifacts/${id}`));
}

/** This viewer's attention on every artifact; {} without a viewer or on failure. */
export async function getAttention(): Promise<Record<string, AttentionSummary>> {
  try { const r = await fetch("/api/viewers/me/attention"); return r.ok ? (await r.json()).artifacts : {}; } catch { return {}; }
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
