import type { FeedbackState, Thread } from "./threads";

export type ArtifactEvent =
  /** A new version; `by_page` when the page published it (`artifact.publish`). */
  | { type: "version"; artifact_id: string; n: number; by_page?: boolean }
  | { type: "artifact_deleted"; artifact_id: string }
  | { type: "thread"; artifact_id: string; thread: Thread }
  | { type: "comment"; artifact_id: string; thread_id: string; comment: unknown }
  | { type: "thread_resolved"; artifact_id: string; thread_id: string; resolved_by: string; resolved_at: string }
  | ({ type: "feedback_state"; artifact_id: string } & FeedbackState)
  /** A shared document changed; `version` is null after a delete. Only
   * documents this stream's viewer may read are announced. */
  | { type: "doc"; artifact_id: string; path: string; version: number | null }
  /** The stream (re)connected; anything published while it was down was missed, so refetch state. */
  | { type: "ready" }
  /** The stream failed and is reconnecting (or gave up): nothing is announced until the next `ready`. */
  | { type: "stream_down" }
  /** The stream dropped events; refetch state. */
  | { type: "resync"; dropped: number };

/** Subscribes to the artifact's events. `token` (the owner shell's, from
 * `/api/token`) goes in the query, since an EventSource cannot send headers;
 * the daemon then counts the stream as the owner shell's. */
export function subscribe(artifactId: string, onEvent: (e: ArtifactEvent) => void, token: string | null = null): () => void {
  const q = new URLSearchParams({ artifact: artifactId });
  if (token) q.set("token", token);
  const es = new EventSource(`/api/events?${q}`);
  const handler = (e: MessageEvent) => { try { onEvent(JSON.parse(e.data)); } catch { /* ignore malformed */ } };
  es.addEventListener("version", handler);
  es.addEventListener("artifact_deleted", handler);
  for (const name of ["thread", "comment", "thread_resolved", "feedback_state", "doc"]) es.addEventListener(name, handler);
  es.addEventListener("ready", () => onEvent({ type: "ready" }));
  es.addEventListener("error", () => onEvent({ type: "stream_down" }));
  es.addEventListener("resync", (e: MessageEvent) => {
    try { onEvent({ type: "resync", dropped: Number(JSON.parse(e.data).dropped) || 0 }); } catch { onEvent({ type: "resync", dropped: 0 }); }
  });
  return () => es.close();
}
