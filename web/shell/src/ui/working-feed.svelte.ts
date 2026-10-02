// The gallery's live working lists, by artifact: seeded from
// `GET /api/artifacts`, then kept current by the `working` event stream,
// which opens only after the gallery has rendered.
import type { Artifact } from "../api";
import type { Working } from "../view/working-model";
import { subscribeWorking } from "../working-events";

export class WorkingFeed {
  byId = $state<Record<string, Working[]>>({});
  /** Counts `working` events; `#at` holds the count at each artifact's latest. */
  events = 0;
  #at: Record<string, number> = {};
  #stop: (() => void) | null = null;
  /** Takes each artifact's list from `list`, fetched when `events` was `since`;
   * an artifact with an event after that keeps its newer list. */
  seed(list: Artifact[], since = this.events): void {
    this.byId = Object.fromEntries(list.map(a => [a.id, (this.#at[a.id] ?? 0) > since ? this.byId[a.id] ?? [] : a.working ?? []]));
  }
  start(onResync: () => void): void {
    if (this.#stop) return;
    this.#stop = subscribeWorking(e => {
      if (e.type === "working") {
        this.#at[e.artifact_id] = ++this.events;
        this.byId = { ...this.byId, [e.artifact_id]: e.working };
      } else if (e.type === "ready" || e.type === "resync") onResync();
    });
  }
  stop(): void { this.#stop?.(); this.#stop = null; }
}
