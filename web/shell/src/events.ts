import type { FeedbackState, Thread } from "./threads";
import type { PresenceView } from "./view/presence-model";
import type { Working } from "./view/working-model";
import type { DraftView } from "./view/draft-model";

/** Every `/api/events` event may name `cause`: the id of the request it was
 * published because of (the daemon's `x-request-id`, which a page may choose
 * itself, see `causedFetch`). Absent for events no request caused. */
export type ArtifactEvent = Caused & ArtifactEventBody;

/** The request an event names as its cause. */
export interface Caused { cause?: string }

/** Requests this page sent with `causedFetch`, by id: what they were for
 * (`label`) and the User Timing mark they started at. Bounded: the oldest
 * is forgotten past `MAX_PENDING`. */
const pending = new Map<string, { label: string; mark: string }>();
const MAX_PENDING = 200;

/** A new request id: well formed for the daemon (`[A-Za-z0-9._-]{1,64}`). */
export function newRequestId(): string {
  const rand = Math.random().toString(36).slice(2, 12);
  return `p${Date.now().toString(36)}-${rand}`;
}

/** Notes request `id` (for `label`) as pending, with a User Timing mark at its start. */
export function noteRequest(id: string, label: string): void {
  const mark = `clax:req:${id}`;
  try { performance.mark(mark, { detail: { id, label } }); } catch { /* no User Timing */ }
  pending.set(id, { label, mark });
  while (pending.size > MAX_PENDING) pending.delete(pending.keys().next().value as string);
}

/** `fetch`, under a request id this page chose: sent as `x-request-id` and
 * noted (`noteRequest`), so the events the request causes are joined back to
 * it (`noteEvent`) and its `Server-Timing` sits beside a User Timing
 * measure in the Performance panel. */
export function causedFetch(label: string, input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const id = newRequestId();
  const headers = new Headers(init.headers);
  headers.set("x-request-id", id);
  noteRequest(id, label);
  return fetch(input, { ...init, headers });
}

/** Joins event `e` to the pending request it names as its cause, if this
 * page sent it: measures request → event in User Timing
 * (`clax <label> → <type>`) and returns what the request was for. */
export function noteEvent(e: Caused & { type: string }): string | undefined {
  const p = e.cause ? pending.get(e.cause) : undefined;
  if (!p) return undefined;
  try { performance.measure(`clax ${p.label} → ${e.type}`, { start: p.mark, detail: { cause: e.cause } }); } catch { /* mark cleared */ }
  return p.label;
}

type ArtifactEventBody =
  /** A new version; `by_page` when the page published it (`artifact.publish`). */
  | { type: "version"; artifact_id: string; n: number; by_page?: boolean }
  | { type: "artifact_deleted"; artifact_id: string }
  | { type: "thread"; artifact_id: string; thread: Thread }
  | { type: "comment"; artifact_id: string; thread_id: string; comment: unknown }
  | { type: "thread_resolved"; artifact_id: string; thread_id: string; resolved_by: string; resolved_at: string }
  /** A thread was deleted with its comments. */
  | { type: "thread_deleted"; artifact_id: string; thread_id: string }
  | ({ type: "feedback_state"; artifact_id: string } & FeedbackState)
  /** A shared document changed; `version` is null after a delete. Only
   * documents this stream's viewer may read are announced. */
  | { type: "doc"; artifact_id: string; path: string; version: number | null }
  /** Who is working on the artifact now: its whole list, newest first. */
  | { type: "working"; artifact_id: string; working: Working[] }
  /** Agents' live drafts on the artifact (hosted spec §16.4): its whole list. */
  | { type: "drafts"; artifact_id: string; drafts: DraftView[] }
  /** An agent that watched the artifact started, ended or was renamed:
   * refetch its agents (the Send menu lists live ones only). Names nothing. */
  | { type: "agents"; artifact_id?: undefined }
  /** The artifact's tags changed (`artifact_id`), or the list renamed,
   * recoloured or deleted one it carries (`artifact_id` null): refetch it. */
  | { type: "tags"; artifact_id: string | null }
  /** The artifact's Comments switch is now `comments` (a sharer turned it). */
  | { type: "comments"; artifact_id: string; comments: boolean }
  /** The artifact's capability approval changed (an approval or a withdrawal): refetch it and its declaration in force. */
  | { type: "permissions"; artifact_id: string }
  /** Who has the artifact open, here or away, or was here lately: its whole list. */
  | { type: "presence"; artifact_id: string; people: PresenceView[] }
  /** The watcher's topics went live: on first subscribing, after the page
   * shows again, or after a reconnect that could not resume. Anything
   * published meanwhile may be missed, so refetch state. */
  | { type: "ready" }
  /** The stream failed and is reconnecting: nothing is announced until `stream_up` or the next `ready`. */
  | { type: "stream_down" }
  /** The stream is back after `stream_down` and resumed: nothing was missed. */
  | { type: "stream_up" }
  /** The stream dropped events of `topic` (the view's when absent); refetch state. */
  | { type: "resync"; topic?: string };
