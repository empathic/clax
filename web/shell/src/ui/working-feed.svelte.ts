// The gallery's live working lists, by artifact: seeded from
// `GET /api/artifacts`, then kept current by the gallery's event stream,
// which opens only after the gallery has rendered. Its `version` and `thread`
// events call `onChange` (which refreshes the viewer's attention) at most
// once a second.
import type { Artifact } from "../api";
import type { Working } from "../view/working-model";
import { subscribeGallery } from "../working-events";

export class WorkingFeed {
  byId = $state<Record<string, Working[]>>({});
  /** Counts `working` events; `#at` holds the count at each artifact's latest. */
  events = 0;
  #at: Record<string, number> = {};
  #stop: (() => void) | null = null;
  #soon: ReturnType<typeof setTimeout> | null = null;
  #last = 0;
  /** Takes each artifact's list from `list`, fetched when `events` was `since`;
   * an artifact with an event after that keeps its newer list. */
  seed(list: Artifact[], since = this.events): void {
    this.byId = Object.fromEntries(list.map(a => [a.id, (this.#at[a.id] ?? 0) > since ? this.byId[a.id] ?? [] : a.working ?? []]));
  }
  start(onResync: () => void, onChange: () => void = () => {}): void {
    if (this.#stop) return;
    this.#stop = subscribeGallery(e => {
      if (e.type === "working") {
        this.#at[e.artifact_id] = ++this.events;
        this.byId = { ...this.byId, [e.artifact_id]: e.working };
      } else if ((e.type === "version" || e.type === "thread") && !this.#soon) {
        this.#soon = setTimeout(() => { this.#soon = null; this.#last = Date.now(); onChange(); }, Math.max(0, this.#last + 1000 - Date.now()));
      }
      else if (e.type === "ready" || e.type === "resync") onResync();
    });
  }
  stop(): void { this.#stop?.(); this.#stop = null; if (this.#soon) clearTimeout(this.#soon); this.#soon = null; }
}
