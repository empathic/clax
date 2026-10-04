// The gallery's cards once its first list has painted: a full refresh when
// the stream opens or resyncs and every `SAFETY_MS` while the page is
// visible, and a refresh of one card (its entry and this viewer's attention
// on it, nothing else) when an event names its artifact. Each refresh takes a
// ticket when it starts; its answer replaces a card's entry, or its
// attention, only where no refresh started later has already done so.
import { type Artifact, type AttentionSummary, getAttention, listArtifacts } from "../api";
import { Lifecycle, onPageCache } from "../lifecycle";
import type { WorkingFeed } from "./working-feed.svelte";

/** How often a visible gallery refetches everything, as a safety net. */
export const SAFETY_MS = 60_000;

type Att = Record<string, AttentionSummary>;
/** The gallery's state: its cards (null before the first list) and attention. */
export type Cards = { list(): Artifact[] | null; att(): Att; setList(l: Artifact[]): void; setAtt(a: Att): void };

/** `old` with each of `keys` taken from `next` (dropped where `next` lacks
 * it) when no answer newer than ticket `t` set it; `tickets` records `t`. */
function merge<V>(tickets: Map<string, number>, t: number, old: Map<string, V>, next: Map<string, V>, keys: Iterable<string>): Map<string, V> {
  const out = new Map(old);
  for (const k of keys) {
    if ((tickets.get(k) ?? 0) > t) continue;
    tickets.set(k, t);
    const v = next.get(k);
    if (v === undefined) out.delete(k);
    else out.set(k, v);
  }
  return out;
}

export class CardSync {
  #n = 0;
  #lists = new Map<string, number>();
  #atts = new Map<string, number>();
  #fullAt = 0;
  #life: Lifecycle | null = null;
  /** The safety timer's own lifecycle: ended while the page is in the back/forward cache. */
  #timer: Lifecycle | null = null;
  constructor(private cards: Cards, private feed: WorkingFeed, private every = SAFETY_MS) {}

  /** Follows the feed's stream; refreshes everything on `ready` and `resync`. */
  start(): void {
    if (this.#life) return;
    const life = (this.#life = new Lifecycle());
    this.feed.start(() => void this.full(), id => void this.one(id));
    life.defer(() => this.feed.stop());
    this.#arm();
    life.listen(document, "visibilitychange", this.#shown);
    // The stream closes and resumes by itself; the timer stops with it.
    onPageCache(life, () => { this.#timer?.dispose(); this.#timer = null; }, () => { this.#arm(); this.#shown(); });
  }
  stop(): void {
    this.#life?.dispose();
    this.#life = null;
    this.#timer = null;
  }
  #arm(): void {
    if (!this.#life || this.#timer) return;
    this.#timer = this.#life.child();
    this.#timer.interval(() => { if (document.visibilityState !== "hidden") void this.full(); }, this.every);
  }
  #shown = () => { if (document.visibilityState === "visible" && Date.now() - this.#fullAt >= this.every) void this.full(); };

  /** Refetches every card and all of this viewer's attention. */
  full(): Promise<void> {
    this.#fullAt = Date.now();
    return this.#fetch(null);
  }
  /** Refetches artifact `id`'s card and this viewer's attention on it; a card
   * whose artifact is no longer live goes. */
  one(id: string): Promise<void> {
    return this.#fetch(id);
  }

  #fetch(id: string | null): Promise<void> {
    const t = ++this.#n;
    const since = this.feed.events;
    const only = id ?? undefined;
    return Promise.all([listArtifacts(only).catch(() => null), getAttention(only)]).then(([list, att]) => {
      if (list) {
        const old = new Map((this.cards.list() ?? []).map(a => [a.id, a]));
        const next = new Map(list.map(a => [a.id, a]));
        const out = merge(this.#lists, t, old, next, id ? [id] : [...old.keys(), ...next.keys()]);
        const l = [...out.values()];
        this.cards.setList(l);
        if (id) this.feed.seedOne(id, out.get(id) ?? null, since);
        else this.feed.seed(l, since);
      }
      if (att) {
        const old = new Map(Object.entries(this.cards.att()));
        const next = new Map(Object.entries(att));
        this.cards.setAtt(Object.fromEntries(merge(this.#atts, t, old, next, id ? [id] : [...old.keys(), ...next.keys()])));
      }
    });
  }
}
