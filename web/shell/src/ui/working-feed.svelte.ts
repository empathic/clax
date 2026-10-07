// The gallery's live working lists, by artifact: seeded from
// `GET /api/artifacts`, then kept current by the `gallery` topic of the
// page's event stream, which the gallery watches only after it has
// rendered. `working` deltas replace an artifact's list. `version` and
// `artifact_deleted` deltas go to `onDelta` at once; `version`, `thread`
// and `thread_deleted` also call `onChange` with the artifact they name, at
// most once a second per artifact: a burst on one artifact is one call, and
// other artifacts do not wait for it.
import type { Artifact } from "../api";
import type { StreamEvent } from "../stream";
import { type WorkingSummary, workingFromSummary } from "../view/deltas";
import type { Working } from "../view/working-model";
import { subscribeGallery } from "../working-events";

/** A `gallery` delta the cards apply themselves. */
export type CardDelta = { type: "version"; artifact_id: string; n: number; title?: string | null; at?: string | null } | { type: "artifact_deleted"; artifact_id: string };

export class WorkingFeed {
  byId = $state<Record<string, Working[]>>({});
  /** Counts `working` events; `#at` holds the count at each artifact's latest. */
  events = 0;
  #at: Record<string, number> = {};
  #stop: (() => void) | null = null;
  #soon = new Map<string, ReturnType<typeof setTimeout>>();
  #last = new Map<string, number>();
  /** Takes each artifact's list from `list`, fetched when `events` was `since`;
   * an artifact with an event after that keeps its newer list. */
  seed(list: Artifact[], since = this.events): void {
    this.byId = Object.fromEntries(list.map(a => [a.id, this.#newer(a.id, since) ? this.byId[a.id] ?? [] : a.working ?? []]));
  }
  /** `seed` for artifact `id` alone, the others kept; `a` null drops it. */
  seedOne(id: string, a: Artifact | null, since = this.events): void {
    if (this.#newer(id, since)) return;
    const next = { ...this.byId };
    if (a) next[id] = a.working ?? [];
    else delete next[id];
    this.byId = next;
  }
  #newer(id: string, since: number): boolean { return (this.#at[id] ?? 0) > since; }
  start(onResync: () => void, onChange: (id: string) => void = () => {}, onDelta: (d: CardDelta) => void = () => {}): void {
    if (this.#stop) return;
    this.#stop = subscribeGallery((e: StreamEvent) => {
      const id = typeof e.artifact_id === "string" ? e.artifact_id : null;
      if (e.type === "working" && id) {
        this.#at[id] = ++this.events;
        this.byId = { ...this.byId, [id]: workingFromSummary((e.working ?? []) as WorkingSummary[]) };
      } else if ((e.type === "version" || e.type === "artifact_deleted") && id) {
        onDelta(e as unknown as CardDelta);
        if (e.type === "artifact_deleted") { const next = { ...this.byId }; delete next[id]; this.byId = next; }
      }
      if ((e.type === "version" || e.type === "thread" || e.type === "thread_deleted") && id) {
        if (this.#soon.has(id)) return;
        const wait = Math.max(0, (this.#last.get(id) ?? 0) + 1000 - Date.now());
        this.#soon.set(id, setTimeout(() => { this.#soon.delete(id); this.#last.set(id, Date.now()); onChange(id); }, wait));
      } else if (e.type === "ready" || e.type === "resync" || e.type === "site") onResync();
    });
  }
  stop(): void {
    this.#stop?.(); this.#stop = null;
    for (const t of this.#soon.values()) clearTimeout(t);
    this.#soon.clear();
  }
}
