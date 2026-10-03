<svelte:options css="injected" />

<script lang="ts">
  // Open threads: those on the page shown and found (numbered like the pins),
  // then those on other pages of the version (labelled "on <file>"). Below,
  // collapsed into a tail: open threads on the page shown and not found, or on
  // a page the version does not hold (Detached); then resolved threads.
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import type { Participants, Version } from "../api";
  import type { Decided } from "../view/changelog-model";
  import { relativeTime } from "../format";
  import { type Thread, type Viewer, resolvedByLabel } from "../threads";
  import { type Selection, selectable, sendLabel, unsent, unsentLabel } from "../view/batch-model";
  import { guardedAction } from "../view/trail";
  import { agentName, historyOf, isOutdated } from "../view/history-model";
  import { needsTicking, sidebarSections } from "../view/sidebar-model";
  import { type Working, agentNames, newestFirst, stripText, threadAgent, threadMarker } from "../view/working-model";
  import SendButton from "./SendButton.svelte";
  import ThreadCard from "./ThreadCard.svelte";
  import { ticker } from "./ticker.svelte";

  type Props = {
    threads: Thread[];
    resolved: Record<string, AnchorResult>;
    /** Fixed clock for tests; without it the sidebar ticks each second while a label shows elapsed time, else every 30 s. */
    now?: Date;
    selected: string | null;
    onSelect(t: Thread): void;
    onSend(t: Thread): void;
    onResolve(t: Thread): void;
    onReply(t: Thread, body: string): void;
    /** Hears the thread whose card the pointer is over, and null when it leaves. */
    onHover?(t: Thread | null): void;
    /** This viewer, to show its own name on threads it resolved. */
    me?: Viewer | null;
    /** The page the frame shows (the index by default; null when the frame shows
     * a document that did not greet); threads on other pages are labelled with theirs. */
    file?: string | null;
    /** Whether the shown version holds a page; a thread on a page it lacks is detached. */
    holds?: (file: string) => boolean;
    /** The artifact's versions, which tag each event in a thread's history. */
    versions: Version[];
    /** The version shown, against which a thread may be outdated. */
    shown: number;
    /** The publishing agent's name. */
    agent: string;
    /** Who is working on the artifact now. */
    working?: Working[];
    /** The artifact's agents, which name the working ones. */
    agents?: Participants["agents"];
    /** Comment mode is on: the working strip leaves its haiku out. */
    commenting?: boolean;
    /** The open threads the viewer is in. */
    mine?: string[];
    /** This load's changelog: its threads lead the sidebar as "Addressed in vN". */
    decided?: Decided | null;
    /** A card has been looked at (decided: Q4). */
    onSeen?(t: Thread): void;
    /** The threads ticked for a batch send (spec §8); open cards get a box when `onToggle` is given. */
    selection?: Selection;
    /** The note the selection bar sends with the batch, and whether a batch send is in flight. */
    batchNote?: string;
    batchBusy?: boolean;
    /** The agent handle every Send goes to; null sends without one. */
    sendTo?: string | null;
    /** Ticks or unticks `t`; `order` is the cards' order here, for a Shift range. */
    onToggle?(t: Thread, shift: boolean, order: string[]): void;
    onClear?(): void;
    onNote?(v: string): void;
    onSendSelection?(): void;
    onSendUnsent?(): void;
    /** The agent picker chose `handle`. */
    onChoose?(handle: string): void;
  };
  let p: Props = $props();
  // Each second while a waiting label counts; otherwise often enough for "N min ago".
  const clock = ticker(() => needsTicking(p.threads) || !!p.working?.length, () => p.now, 30_000);
  const s = $derived(sidebarSections(p.threads, p.resolved, p.file, p.holds));
  // The group holds this load's decision, open or resolved since (decided: Q4).
  const group = $derived(p.decided ? p.threads.filter(t => p.decided!.ids.includes(t.id)) : []);
  const rest = (list: Thread[]) => (group.length ? list.filter(t => !p.decided!.ids.includes(t.id)) : list);
  const open = $derived(rest(s.open));
  const detached = $derived(rest(s.detached));
  const resolvedRest = $derived(rest(s.resolved));
  const agentsByHandle = $derived(agentNames(p.working ?? [], p.agents ?? []));
  const names = (t: Thread) => (by: string) => by.startsWith("agent:") ? agentName(by.slice(6)) : resolvedByLabel(by, p.me, t.resolved_by_name);
  // A collapsed group opens when the selected card newly enters it (a new
  // selection, or the selected card moving there), or when the card the
  // viewer just resolved arrives there within RESOLVE_OPEN_MS. A group the
  // viewer collapsed stays collapsed while the same card stays in it.
  const RESOLVE_OPEN_MS = 5000;
  let detachedOpen = $state(false);
  let resolvedOpen = $state(false);
  let justResolved: { id: string; until: number } | null = null;
  const openedFor = { detached: null as string | null, resolved: null as string | null };
  const has = (list: Thread[], id: string | null) => !!id && list.some(t => t.id === id);
  $effect(() => {
    const sel = p.selected;
    if (has(detached, sel)) { if (openedFor.detached !== sel) { detachedOpen = true; openedFor.detached = sel; } } else openedFor.detached = null;
    if (has(resolvedRest, sel)) { if (openedFor.resolved !== sel) { resolvedOpen = true; openedFor.resolved = sel; } } else openedFor.resolved = null;
    if (justResolved && has(resolvedRest, justResolved.id)) {
      if (Date.now() <= justResolved.until) resolvedOpen = true;
      justResolved = null;
    }
  });
  // Every Send names the same agent: the target's name, else the publishing agent's.
  const target = $derived(agentsByHandle.get(p.sendTo ?? "") ?? p.agent);
  const ticked = $derived(new Set(p.selection?.ids ?? []));
  // The cards in the order shown, for a Shift range: the Addressed group, Open, then Detached.
  const order = $derived([...group, ...open, ...detached].filter(t => selectable(t, false)).map(t => t.id));
  const toggleCard = (t: Thread, shift: boolean) => p.onToggle?.(t, shift, order);
  const unsentCount = $derived(unsent(p.threads).length);
  let unsentHint: string | null = $state(null);
  const resolve = (t: Thread) => { justResolved = { id: t.id, until: Date.now() + RESOLVE_OPEN_MS }; p.onResolve(t); };
</script>

{#snippet cards(list: Thread[])}
  {#each list as t (t.id)}
    <ThreadCard {t} n={s.numbers.get(t.id)} now={clock.now} me={p.me} selected={p.selected} file={s.file}
      history={historyOf(t, p.versions, names(t), { working: t.status === "open" ? threadAgent(p.working ?? [], t.id, agentsByHandle) : null })}
      marker={t.status === "open" ? threadMarker(p.working ?? [], t.id, agentsByHandle) : null} outdated={isOutdated(t, p.resolved[t.id], p.shown)} agent={p.agent} when={relativeTime(t.created_at, clock.now)}
      versions={p.versions} onSeen={p.onSeen}
      checked={ticked.has(t.id)} onToggle={p.onToggle && selectable(t, false) ? toggleCard : undefined}
      onSelect={p.onSelect} onSend={p.onSend} onResolve={resolve} onReply={p.onReply} onHover={p.onHover}>
      {#snippet send(guard: (e: Event, act: () => void) => void)}
        <SendButton label={`Send to ${target}`} agents={p.agents ?? []} names={agentsByHandle} target={p.sendTo ?? null}
          onSend={e => guard(e, () => p.onSend(t))} onChoose={h => p.onChoose?.(h)} />
      {/snippet}
    </ThreadCard>
  {/each}
{/snippet}

<aside class="sidebar" aria-label="Comment threads">
  {#each newestFirst(p.working ?? []) as w (w.key)}
    {#await import("./WorkingStrip.svelte") then { default: WorkingStrip }}
      <WorkingStrip {w} text={stripText(w, agentsByHandle, s.numbers, new Set(p.mine ?? []))} commenting={p.commenting ?? false} />
    {/await}
  {/each}
  {#if unsentCount && p.onSendUnsent}
    <button type="button" class="primary send-unsent" disabled={p.batchBusy}
      onclick={e => { unsentHint = guardedAction(e, "send", () => p.onSendUnsent?.()); }}>{unsentLabel(unsentCount, target)}</button>
    <p class="act-hint" role="status">{unsentHint ?? ""}</p>
  {/if}
  {#if p.selection?.ids.length}
    {#await import("./SelectionBar.svelte") then { default: SelectionBar }}
      <SelectionBar count={p.selection.ids.length} note={p.batchNote ?? ""} busy={p.batchBusy ?? false}
        onNote={v => p.onNote?.(v)} onClear={() => p.onClear?.()} onSend={() => p.onSendSelection?.()}>
        {#snippet send(go: (e: Event) => void)}
          <SendButton label={sendLabel(p.selection!.ids.length, target)} agents={p.agents ?? []} names={agentsByHandle} target={p.sendTo ?? null}
            disabled={p.batchBusy} onSend={go} onChoose={h => p.onChoose?.(h)} />
        {/snippet}
      </SelectionBar>
    {/await}
  {/if}
  {#if group.length}
    {#await import("./AddressedGroup.svelte") then { default: AddressedGroup }}
      <AddressedGroup n={p.decided!.n} agent={p.agent} count={group.length}>{@render cards(group)}</AddressedGroup>
    {/await}
  {/if}
  <section class="section-open">
    <h2 class="gh you"><span class="sw" aria-hidden="true"></span><span class="t">Open</span> <span class="c">{open.length}</span></h2>
    {#if open.length === 0}<p class="muted small empty-open">{group.length ? "Nothing else open." : "Nothing open. Press C and click anything to comment on it."}</p>{:else}{@render cards(open)}{/if}
  </section>
  <div class="tail">
    <details class="section-detached" aria-label={`Detached ${detached.length}`} bind:open={detachedOpen}>
      <summary><h2 class="gh oth"><span class="sw" aria-hidden="true"></span><span class="t">Detached</span> <span class="c">{detached.length}</span></h2></summary>
      {@render cards(detached)}
    </details>
    <details class="section-resolved" aria-label={`Resolved ${resolvedRest.length}`} bind:open={resolvedOpen}>
      <summary><h2 class="gh set"><span class="sw" aria-hidden="true"></span><span class="t">Resolved</span> <span class="c">{resolvedRest.length}</span></h2></summary>
      {@render cards(resolvedRest)}
    </details>
  </div>
</aside>

<!-- The sidebar's styles (spec §8, "Thread sidebar") travel with its lazy
     chunk (injected on mount), so the artifact entry's eager CSS does not
     carry them. The sidebar's box and the thread number's rule are the theme's: the
     box holds the sidebar's width before this chunk loads, and the number is
     shared with the pins. -->
<style>
  :global {
    .gh { display: flex; align-items: center; gap: 8px; margin: 6px 2px 8px; font: 600 19px/1.1 var(--grot); list-style: none; cursor: default; }
    .tail summary { list-style: none; cursor: pointer; }
    .tail summary::-webkit-details-marker { display: none; }
    summary .gh { font-size: 16px; color: var(--muted); cursor: pointer; }
    .gh .t { flex: 1; } .gh .c { font: 400 12px var(--mono); color: var(--muted); }
    .gh .sw { width: 7px; height: 16px; flex: none; }
    .gh.you .sw { border-radius: 0 8px 8px 0; background: var(--you); }
    .gh.ag .sw { border-radius: 8px 0 0 8px; background: var(--agent); } .gh.ag .t { color: var(--agent-ink); }
    .gh.oth .sw { border-radius: 0 8px 8px 0; box-shadow: inset 0 0 0 1.5px var(--you); }
    .gh.set .sw { border-radius: 50%; width: 10px; height: 10px; background: var(--border-strong); }
    .tail { margin-top: 6px; border-top: 1px solid var(--border); padding-top: 8px; }
    .thread-card { background: var(--card); border: 1px solid var(--border); padding: 11px 12px 10px; margin-bottom: 10px; cursor: pointer; transition: background-color var(--t); }
    .thread-card:hover { border-color: var(--border-strong); }
    .thread-card.selected { box-shadow: inset 3px 0 0 var(--accent); background: var(--comment-hl); }
    .thread-card header { margin-bottom: 8px; min-width: 0; }
    .thread-card .card-head { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; min-height: 0; background: none; border: 0; padding: 0; color: var(--muted); text-align: left; font: 400 12px var(--mono); }
    .thread-card .anchor-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg); }
    .thread-card .thumb { display: block; max-width: 100%; max-height: 120px; min-height: 24px; margin-bottom: 6px; object-fit: cover; object-position: left top; border: 1px solid var(--border); background: var(--bg); }
    .vt { font: 600 11px/15px var(--grot); padding: 0 4px; border: 1px solid var(--border-strong); color: var(--fg); background: var(--bg); white-space: nowrap; }
    .vt.out { color: var(--muted); }
    .msg { display: grid; gap: 3px; }
    .msg + .msg { margin-top: 10px; }
    .msg .author { font: 600 14px/20px var(--grot); }
    .msg .body { margin: 0; font-size: 13.5px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
    .msg.you { border-left: 3px solid var(--you); padding: 1px 0 1px 10px; }
    .msg.agent { border-right: 3px solid var(--agent); padding: 1px 10px 1px 0; margin-left: 26px; text-align: right; }
    .msg.agent .author { color: var(--agent-ink); }
    .msg.agent .body { text-align: left; }
    .st { display: flex; align-items: center; gap: 8px; margin: 10px 0 0; padding-top: 8px; border-top: 1px dashed var(--border); font-size: 12px; color: var(--muted); }
    .st.ag { color: var(--agent-ink); font: 600 13.5px/1.2 var(--grot); } .st small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: auto; }
    .hist { display: flex; flex-wrap: wrap; gap: 4px 6px; margin: 9px 0 0; padding: 8px 0 0; list-style: none; border-top: 1px dashed var(--border); font-size: 11.5px; color: var(--muted); line-height: 1.6; }
    .hist .sep { margin-right: 2px; }
    .hist .ev { white-space: nowrap; }
    /* Each version's events start a line; the "·" stays in the text, read and copied. */
    .hist .br { flex-basis: 100%; height: 0; }
    .hist .nl .sep { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
    .hist .ev b { font-weight: 600; color: var(--fg); }
    .hist .ev.agent .vt { border-color: var(--agent); color: var(--agent-ink); }
    .thread-card .actions { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; }
    .act-hint { margin: 6px 0 0; font-size: 12px; color: var(--muted); text-align: right; }
    .act-hint:empty { margin: 0; }
    .reply { display: flex; gap: 6px; margin-top: 8px; }
    .reply input { flex: 1; min-width: 0; }
    /* Batch send (spec §8): the cards' boxes, the shared Send and its agent picker. */
    .thread-card header { display: flex; align-items: center; gap: 8px; }
    .thread-check { width: 18px; height: 18px; margin: 0; accent-color: var(--accent); flex: none; cursor: pointer; }
    .thread-card:has(.thread-check:checked) { box-shadow: inset 0 0 0 1px var(--accent); }
    .send { display: inline-flex; position: relative; }
    .send .caret { min-width: 28px; padding: 0 6px; border-left: 1px solid color-mix(in srgb, var(--on-accent) 35%, transparent); font-family: var(--mono); }
    .send-menu { position: absolute; right: 0; top: calc(100% + 6px); z-index: 20; min-width: 160px; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); display: flex; flex-direction: column; padding: 4px 0; }
    .send-menu button { justify-content: flex-start; border: 0; min-height: 32px; }
    .send-menu button[aria-checked="true"]::before { content: "✓"; margin-right: 6px; }
    .send-unsent { width: 100%; margin-bottom: 4px; }
    @media (max-width: 700px) { .send-menu button, .send-unsent { min-height: 40px; } }
  }
</style>
