// Connection requests' routes (spec 2026-10-29-remote-instances §3): the
// owner's answer to an agent's request to connect another Clax, and the
// muted instances Settings lists. Every change is the owner's browser's
// (its cookie, never the token); a refusal throws `Refused` with the
// daemon's own message.
import type { InboxRemoteRequest } from "../api";
import { Refused } from "../people/api";

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const res = await fetch(path, method === "GET" ? { cache: "no-store" } : { method, ...body !== undefined && { headers: { "content-type": "application/json" }, body: JSON.stringify(body) } });
  if (res.ok) return (await res.json()) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}

/** The owner's answer to a request. */
export type RemoteAnswer = "connect" | "not_now" | "never";

/** What answering says: the request after it, and for `connect` the
 * sign-in to approve on the other instance (`verification_url`). */
export type Answered = { request: { status: InboxRemoteRequest["status"] }; verification_url?: string; user_code?: string };

/** Answers connection request `id`. */
export const answerRemote = (id: string, answer: RemoteAnswer) =>
  call<Answered>(`/api/remotes/requests/${encodeURIComponent(id)}/answer`, "POST", { answer });

/** A muted Clax instance: its origin and when it was muted. */
export type RemoteMute = { origin: string; muted_at: string };

/** The instances muted with "Never for this instance", newest first. */
export const remoteMutes = () => call<{ mutes: RemoteMute[] }>("/api/remotes/mutes");

/** Unmutes `origin`: its agents may ask again. */
export const unmuteRemote = (origin: string) =>
  call<{ origin: string; unmuted: boolean }>(`/api/remotes/mutes/${encodeURIComponent(origin)}`, "DELETE");
