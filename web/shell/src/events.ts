import type { FeedbackState, Thread } from "./threads";
import type { PresenceView } from "./view/presence-model";
import type { Working } from "./view/working-model";

export type ArtifactEvent =
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
