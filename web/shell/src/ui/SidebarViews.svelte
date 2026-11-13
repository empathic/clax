<svelte:options css="injected" />

<script lang="ts" module>
  /** The timeline's rows open into their cards, by thread ID, for the page's life. */
  let expanded: string[] = $state([]);
  const NONE: never[] = [];
</script>

<script lang="ts">
  // The sidebar's two views (spec 2026-11-13, "Two views"): the Timeline |
  // Threads tabs, directly above the list they switch, and the list: the
  // timeline (Timeline.svelte) or the thread list's sections, which the
  // sidebar gives as `sections`. The tabs' right end says how many threads
  // wait on a person, and selects the newest of them. Loaded apart from the
  // sidebar (Sidebar.svelte's `view`), so the extension's side panel, which
  // shows the thread list alone, carries none of this; the timeline is
  // computed here, once, for the tabs and the list.
  import type { Snippet } from "svelte";
  import type { Version } from "../api";
  import type { Thread, Viewer } from "../threads";
  import { timeline } from "../view/timeline-model";
  import type { Working } from "../view/working-model";
  import type { SidebarView } from "./sidebar-view.svelte";
  import Timeline from "./Timeline.svelte";

  type Props = {
    /** The view shown, its switch, and the sidebar's outer edge (right by default). */
    view: { now: SidebarView; onChange(v: SidebarView): void; side?: "left" | "right" };
    threads: Thread[]; versions: Version[]; working?: Working[]; claimed?: readonly string[]; me?: Viewer | null;
    /** The sidebar's clock. */
    now: Date;
    selected: string | null;
    /** The pins' numbers; the threads the thread list puts under Detached. */
    numbers: Map<string, number>; detached: Thread[];
    /** What goes above a thread's row, open or not. */
    above?: Snippet<[Thread, Date]>;
    onSeen?(t: Thread): void; onSelect(t: Thread): void;
    /** A thread's card under its row, given the Shift-range order there and the row's close. */
    card: Snippet<[Thread, { order: string[]; close(): void }]>;
    /** The thread list's sections, the empty Open list's hint, and why the
     * cards have no reply box when the sections say it above their cards
     * (said above the timeline then; "" when they do not). */
    sections: Snippet; empty: Snippet; note?: string;
  };
  let p: Props = $props();
  const view = $derived(p.view);
  // Each input on its own, so the timeline is computed again only when one
  // of them is a new value, not on every change of the state they come from.
  const threads = $derived(p.threads);
  const versions = $derived(p.versions);
  const working = $derived(p.working ?? NONE);
  const claimed = $derived(p.claimed ?? NONE);
  const me = $derived(p.me);
  const tl = $derived(timeline({ threads, versions, working, claimed, me }));
  const detached = $derived(new Set(p.detached.map(t => t.id)));
  const VIEWS = ["timeline", "threads"] as const;
  const LABELS = { timeline: "Timeline", threads: "Threads" };
  // On a phone the sidebar is a sheet whose outer edge is its right.
  let phone = $state(false);
  $effect(() => {
    if (typeof matchMedia !== "function") return;
    const m = matchMedia("(max-width: 700px)");
    const on = () => { phone = m.matches; };
    on();
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  });
  // Arrow keys, Home and End move between the tabs, each choosing the view it names.
  const key = (e: KeyboardEvent, i: number) => {
    const j = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? VIEWS.length - 1 : null;
    if (j === null) return;
    e.preventDefault();
    const v = VIEWS[(j + VIEWS.length) % VIEWS.length];
    view.onChange(v);
    document.getElementById(`sb-tab-${v}`)?.focus();
  };
  const needs = () => { const t = p.threads.find(x => x.id === tl.waiting[0]); if (t) p.onSelect(t); };
  const expand = (id: string, o: boolean) => { expanded = [...expanded.filter(x => x !== id), ...(o ? [id] : [])]; };
</script>

<div class="sb-tabs">
  <div role="tablist" aria-label="View">
    {#each VIEWS as v, i (v)}
      <button type="button" role="tab" class="sb-tab" id={`sb-tab-${v}`} aria-selected={view.now === v} aria-controls="sb-panel" tabindex={view.now === v ? 0 : -1}
        onclick={() => view.onChange(v)} onkeydown={e => key(e, i)}>{LABELS[v]}</button>
    {/each}
  </div>
  {#if tl.waiting.length}<button type="button" class="tl-needs" onclick={needs}>{tl.waiting.length} {tl.waiting.length === 1 ? "needs" : "need"} you</button>{/if}
</div>
<div role="tabpanel" class="sb-panel" id="sb-panel" aria-labelledby={`sb-tab-${view.now}`}>
  {#if view.now === "timeline"}
    {#if p.note}<p class="muted small no-reply">{p.note}</p>{/if}
    <Timeline {tl} threads={p.threads} now={p.now} selected={p.selected} numbers={p.numbers} {detached} side={phone ? "right" : view.side ?? "right"}
      {expanded} onExpand={expand} card={p.card} above={p.above} onSeen={p.onSeen} empty={p.empty} />
  {:else}
    {@render p.sections()}
  {/if}
</div>

<style>
  :global {
    .sb-tabs { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 0 16px; border-bottom: 1px solid var(--border); }
    .sb-tabs [role="tablist"] { display: flex; gap: 16px; }
    .sb-tab, .tl-needs { min-height: 32px; padding: 6px 0; background: none; border: 0; border-radius: 0; }
    .sb-tab { margin-bottom: -1px; border-bottom: 2px solid transparent; font: 600 14px/1.2 var(--font); color: var(--muted); }
    .sb-tab:not(:disabled):hover { background: none; border-color: transparent; color: var(--fg); }
    .sb-tab[aria-selected="true"], .sb-tab[aria-selected="true"]:not(:disabled):hover { color: var(--fg); border-bottom-color: var(--fg); }
    .tl-needs { margin-left: auto; font: 500 13px/1.2 var(--font); color: var(--you-ink); }
    .tl-needs:not(:disabled):hover { background: none; text-decoration: underline; }
    .sb-panel { display: flex; flex-direction: column; gap: 10px; }
    @media (max-width: 700px) { .sb-tab, .tl-needs { min-height: 40px; } }
  }
</style>
