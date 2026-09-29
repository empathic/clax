import type { Anchor } from "../../bridge/src/protocol";
import { ApiError } from "./api";

export type Tier = "piggyback" | "stop_hook" | "prompt_hook" | "wait" | "queue" | "inject";
export type FeedbackState = { thread_id: string; state: "sent" | "delivered" | "acknowledged" | "agent_ended"; tier: Tier | null; since: string; resends: number; exhausted: boolean };
export type Comment = { id: string; thread_id: string; author_kind: "viewer" | "agent"; author_name: string; via_harness: string | null; body: string; created_at: string };
export type Thread = {
  id: string; artifact_id: string; version_n: number; anchor: Anchor; status: "open" | "resolved"; sent_to_agent: boolean;
  has_clip: boolean; clip_url: string | null; created_at: string; resolved_at: string | null; resolved_by: string | null;
  comments: Comment[]; feedback_state: FeedbackState | null;
};
/** The daemon's view of this viewer; `public_id` names it in `resolved_by`, the cookie never leaves the daemon. */
export type Viewer = { public_id: string; display_name: string | null; created_at: string };

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
const withToken = (token: string | null): Record<string, string> => (token ? { authorization: `Bearer ${token}` } : {});
/** Reopens a thread. The daemon lets a named viewer or the owner shell (its
 * `token`) do it; anyone else gets 403 `forbidden`. */
export async function reopenThread(aid: string, tid: string, token: string | null = null): Promise<Thread> {
  return (await ok<{ thread: Thread }>(await fetch(`/api/artifacts/${aid}/threads/${tid}/reopen`, { method: "POST", headers: withToken(token) }))).thread;
}
/** Deletes a thread with its comments and clip; the same caller rule as `reopenThread`. */
export async function deleteThread(aid: string, tid: string, token: string | null = null): Promise<void> {
  await ok<unknown>(await fetch(`/api/artifacts/${aid}/threads/${tid}`, { method: "DELETE", headers: withToken(token) }));
}
let viewerMemo: Promise<Viewer> | null = null;
const viewerListeners = new Set<(v: Viewer) => void>();

/** Calls `fn` after every successful viewer lookup or rename; returns the unsubscriber. */
export function onViewer(fn: (v: Viewer) => void): () => void {
  viewerListeners.add(fn);
  return () => { viewerListeners.delete(fn); };
}

const announceViewer = (v: Viewer) => { for (const fn of [...viewerListeners]) fn(v); };

async function fetchViewer(): Promise<Viewer> {
  return (await ok<{ viewer: Viewer }>(await fetch("/api/viewers/me"))).viewer;
}

/** The viewer behind the cookie, fetched once per page: the first request sets
 * the cookie, so every caller shares it. A failed lookup is retried next call. */
export function getViewer(): Promise<Viewer> {
  if (!viewerMemo) {
    const p = fetchViewer();
    viewerMemo = p;
    p.then(announceViewer, () => { if (viewerMemo === p) viewerMemo = null; });
  }
  return viewerMemo;
}

export async function setViewerName(name: string): Promise<Viewer> {
  const v = (await ok<{ viewer: Viewer }>(await fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: name }) }))).viewer;
  viewerMemo = Promise.resolve(v);
  announceViewer(v);
  return v;
}

/** The viewer as capabilities see it: public ID and current name. */
export async function currentViewer(): Promise<{ publicId: string; name: string | null }> {
  const v = await getViewer();
  return { publicId: v.public_id, name: v.display_name };
}

/** Forgets the fetched viewer (tests). */
export function forgetViewer(): void {
  viewerMemo = null;
}

/** `threads` with `t` replacing the thread of the same ID, or appended. */
export function upsert(threads: Thread[], t: Thread): Thread[] {
  const i = threads.findIndex(x => x.id === t.id);
  if (i < 0) return [...threads, t];
  const copy = threads.slice();
  copy[i] = t;
  return copy;
}

/** Who resolved a thread, from its `resolved_by` (`viewer:<public_id>`,
 * `viewer:anonymous`, or `agent:<harness>`): this viewer's own name when `me`
 * is the resolver and has one, "Viewer" for any other viewer, and
 * "Agent · via <harness>" for an agent. */
export function resolvedByLabel(by: string, me?: Viewer | null): string {
  if (by.startsWith("agent:")) return `Agent · via ${by.slice("agent:".length)}`;
  if (me?.display_name && by === `viewer:${me.public_id}`) return me.display_name;
  return "Viewer";
}

/** A short label for an anchor: the quote in «», else the selector. */
export function anchorLabel(a: Anchor): string {
  const q = a.quote?.replace(/\s+/g, " ").trim();
  if (q) return `«${q.length > 80 ? `${q.slice(0, 80)}…` : q}»`;
  return a.kind === "custom" ? `custom: ${a.custom_name ?? ""}` : a.selector ?? "";
}
