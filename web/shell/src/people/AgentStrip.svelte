<svelte:options css="injected" />

<script lang="ts">
  // The inbox's agent strip (spec 2026-10-08-multi-agent-multi-person §13,
  // §26 (c)): chips above the inbox, one per agent with unread items or a
  // block on you, blocked-longest first: "clax-claude · waiting on you
  // 12 min (1 question)", "web · 1 approval for you · 4 min" (an approval
  // request alone may not hold the agent up: spec 2026-10-08 §13 counts a
  // question or an `ask` as waiting), "web · 3 unread", "pi · ended".
  // Another person's agent names its person ("sam-notes · Sam"). A chip filters
  // the inbox by its agent; pressed again, it clears the filter. Names are
  // agents' text, rendered as text. An agent filtered by that has since
  // left the strip keeps a chip of its own, which clears the filter.
  // At phone width the chips sit behind one **Filter by agent** button,
  // which says how many agents there are, how many wait on you, and which
  // one filters the inbox, so the items come first on the screen; picking a
  // chip there folds them again.
  import type { StripAgent } from "../api";
  import { blockedOn, stripOrder, waitedFor } from "./strip";

  let { agents, now, selected, onPick }: { agents: StripAgent[]; now: Date; selected: string | null; onPick(handle: string | null): void } = $props();
  const list = $derived(stripOrder(agents));
  const id = $props.id();
  let open = $state(false);
  const waiting = $derived(list.filter(a => a.blocked).length);
  const picked = $derived(selected ? (list.find(a => a.handle === selected) ?? null) : null);
  const pick = (handle: string | null) => { open = false; onPick(handle); };
</script>

{#if list.length || selected}
  <button type="button" class="astrip-toggle" aria-expanded={open} aria-controls="{id}-agents" data-pressed={selected ? "" : undefined} onclick={() => { open = !open; }}
    >{#if selected}Agent: <b>{picked ? (picked.name ?? picked.harness) : "this agent"}</b>{:else}Filter by agent{/if}{" "}<span class="n"
      >({list.length} {list.length === 1 ? "agent" : "agents"}{waiting ? ` · ${waiting} waiting on you` : ""})</span></button>
  <div class="astrip" id="{id}-agents" role="group" aria-label="Agents" data-open={open ? "" : undefined}>
    {#each list as a (a.handle)}
      <button type="button" class="achip" data-blocked={a.blocked ? "" : undefined} aria-pressed={a.handle === selected}
        onclick={() => pick(a.handle === selected ? null : a.handle)}>
        <b>{a.name ?? a.harness}</b
        >{#if a.via}{" "}<span class="aperson">· via {a.via}</span>{:else if a.own === false && a.person_name}{" "}<span class="aperson">· {a.person_name}</span>{/if
        }{#if a.blocked?.questions}{" "}<span>· waiting on you {waitedFor(a.blocked.since, now)}</span>{" "}<small>({blockedOn(a.blocked)})</small
        >{:else if a.blocked}{" "}<span>· {blockedOn(a.blocked)} for you · {waitedFor(a.blocked.since, now)}</span
        >{:else}{#if !a.live}{" "}<span>· ended</span>{/if}{#if a.unread}{" "}<span>· {a.unread} unread</span>{/if}{/if}</button>
    {/each}
    {#if selected && !list.some(a => a.handle === selected)}
      <button type="button" class="achip" aria-pressed="true" onclick={() => pick(null)}><b>This agent</b>{" "}<span>· clear the filter</span></button>
    {/if}
  </div>
{/if}

<style>
  :global {
    .astrip { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 14px; min-width: 0; }
    .astrip .achip { display: inline-flex; flex-wrap: wrap; justify-content: flex-start; text-align: start; align-items: baseline; gap: 0 5px; max-width: 100%; min-height: 30px; padding: 4px 12px; border-radius: 999px; font: 400 12.5px var(--font); color: var(--muted); background: var(--card); border: 1px solid var(--border-hover); }
    .astrip .achip b { font-weight: 600; color: var(--agent-ink); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .astrip .achip span, .astrip .achip small { white-space: nowrap; }
    .astrip .achip .aperson { white-space: normal; overflow-wrap: anywhere; min-width: 0; }
    .astrip .achip small { font-size: 12px; }
    .astrip .achip[data-blocked] { border-color: var(--agent); color: var(--fg); }
    .astrip .achip[aria-pressed="true"] { background: var(--hover); border-color: var(--fg); color: var(--fg); }
    .astrip-toggle { display: none; }
    @media (max-width: 700px) {
      .astrip-toggle { display: inline-flex; flex-wrap: wrap; justify-content: flex-start; gap: 0 5px; max-width: 100%; margin-top: 12px; text-align: start; }
      .astrip-toggle b { font-weight: 600; color: var(--agent-ink); overflow-wrap: anywhere; }
      .astrip-toggle .n { color: var(--muted); font-weight: 400; }
      .astrip-toggle[data-pressed] { border-color: var(--fg); }
      .astrip:not([data-open]) { display: none; }
      .astrip { margin-top: 8px; }
    }
  }
</style>
