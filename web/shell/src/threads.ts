import type { Anchor } from "../../bridge/src/protocol";
import { ApiError } from "./api";

export type Tier = "piggyback" | "stop_hook" | "prompt_hook" | "wait" | "queue" | "inject";
export type FeedbackState = { thread_id: string; state: "sent" | "delivered" | "acknowledged" | "agent_ended"; tier: Tier | null; since: string; resends: number; exhausted: boolean };
export type Comment = { id: string; thread_id: string; author_kind: "viewer" | "agent"; author_name: string; via_session_id: string | null; body: string; created_at: string };
export type Thread = {
  id: string; artifact_id: string; version_n: number; anchor: Anchor; status: "open" | "resolved"; sent_to_agent: boolean;
  has_clip: boolean; clip_url: string | null; created_at: string; resolved_at: string | null; resolved_by: string | null;
  comments: Comment[]; feedback_state: FeedbackState | null;
};
export type Viewer = { id: string; display_name: string | null; created_at: string };

async function ok<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not JSON */ }
    throw new ApiError(res.status, msg);
  }
  return res.json() as Promise<T>;
}
const post = (url: string, body?: unknown) =>
  fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });

/** Every thread of the artifact, resolved ones included, oldest first. */
export async function listThreads(aid: string): Promise<Thread[]> {
  const out: Thread[] = [];
  let cursor: string | null = null;
  do {
    const q = new URLSearchParams({ include_resolved: "true", limit: "200" });
    if (cursor) q.set("cursor", cursor);
    const page: { threads: Thread[]; next_cursor: string | null } = await ok(await fetch(`/api/artifacts/${aid}/threads?${q}`));
    out.push(...page.threads);
    cursor = page.next_cursor;
  } while (cursor);
  return out;
}

export async function createThread(aid: string, input: { anchor: Anchor; body: string; version: number; clip: Blob | null }): Promise<{ thread: Thread; clip_error?: string }> {
  const form = new FormData();
  form.set("anchor", JSON.stringify(input.anchor));
  form.set("body", input.body);
  form.set("version", String(input.version));
  if (input.clip) form.set("clip", input.clip, "clip.png");
  return ok(await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: form }));
}
export async function addComment(aid: string, tid: string, body: string): Promise<Thread> {
  return (await ok<{ thread: Thread }>(await post(`/api/artifacts/${aid}/threads/${tid}/comments`, { body }))).thread;
}
export async function sendToAgent(aid: string, tid: string): Promise<Thread> {
  return (await ok<{ thread: Thread }>(await post(`/api/artifacts/${aid}/threads/${tid}/send`))).thread;
}
export async function resolveThread(aid: string, tid: string): Promise<Thread> {
  return (await ok<{ thread: Thread }>(await post(`/api/artifacts/${aid}/threads/${tid}/resolve`))).thread;
}
export async function getViewer(): Promise<Viewer> {
  return (await ok<{ viewer: Viewer }>(await fetch("/api/viewers/me"))).viewer;
}
export async function setViewerName(name: string): Promise<Viewer> {
  return (await ok<{ viewer: Viewer }>(await fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: name }) }))).viewer;
}

/** `threads` with `t` replacing the thread of the same ID, or appended. */
export function upsert(threads: Thread[], t: Thread): Thread[] {
  const i = threads.findIndex(x => x.id === t.id);
  if (i < 0) return [...threads, t];
  const copy = threads.slice();
  copy[i] = t;
  return copy;
}

/** A short label for an anchor: the quote in «», else the selector. */
export function anchorLabel(a: Anchor): string {
  const q = a.quote?.replace(/\s+/g, " ").trim();
  if (q) return `«${q.length > 80 ? `${q.slice(0, 80)}…` : q}»`;
  return a.kind === "custom" ? `custom: ${a.custom_name ?? ""}` : a.selector ?? "";
}
