<svelte:options css="injected" />

<script lang="ts">
  // Participants as Echo draws them (spec §8): people open toward the centre
  // from the left in red-orange, agents from the right in green, the page a
  // dot between. The viewer is nearest the centre and underlined. A person
  // present (`presence`, by public ID) carries a here dot, or is dimmed when
  // away or gone (spec §10, "Presence"). Props are
  // read off `p` and every class is static, so the gallery's lazy working
  // module shares no prop or class runtime with the artifact entry.
  import type { Participants } from "../api";
  import type { Working } from "../view/working-model";
  import { agentMark, shownName } from "../view/agent-display";
  import { midName } from "../view/working-model";

  type Props = { people: Participants["people"]; agents: Participants["agents"]; working: Working[]; me: string | null; max: number; small?: boolean; presence?: Map<string, "here" | "away" | "gone"> };
  const p: Props = $props();
  const initials = (n: string | null) => {
    const w = (n ?? "?").trim().split(/\s+/);
    return (w.length > 1 ? w[0][0] + w[1][0] : w[0].slice(0, 2)).toUpperCase();
  };
  const mark = (a: Props["agents"][number]) => agentMark(a.name, a.harness);
  // Its name, with its person's when that person is not this viewer; another
  // Clax's agent with whose Clax it relays from.
  const named = (a: Props["agents"][number]) => a.via ? shownName(a.name || a.harness, a.via) : `${a.name || a.harness}${a.person && a.person.public_id !== p.me ? ` · ${midName(a.person.name)}` : ""}`;
  const ordered = $derived([...p.people].sort((a, b) => Number(b.public_id === p.me) - Number(a.public_id === p.me)));
  const busy = $derived(new Set(p.working.map(w => w.agent)));
  // A live agent that no comment wakes while idle (`wakes: false`) is drawn
  // dashed, and says so in its title.
  const asleep = (a: Props["agents"][number]) => a.live && a.wakes === false;
  const NOWAKE = " · won't be woken by comments until it next acts";
  // One woken only through a Claude Code channel, which Claude Code does
  // not confirm (`wakes: "unsure"`), is drawn dotted.
  const unsure = (a: Props["agents"][number]) => a.live && a.wakes === "unsure";
  const MAYWAKE = " · may not be woken by comments: Claude Code does not confirm its channel";
</script>

{#snippet roster()}
  <span class="side ppl">
    {#each ordered.slice(0, p.max) as v (v.public_id)}
      {@const at = p.presence?.get(v.public_id)}
      {#if v.public_id === p.me}
        {#if at === "here"}<span class="tok p me here" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>
        {:else if at}<span class="tok p me away" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>
        {:else}<span class="tok p me" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>{/if}
      {:else if at === "here"}<span class="tok p here" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>
      {:else if at}<span class="tok p away" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>
      {:else}<span class="tok p" role="img" title={v.display_name ?? "Viewer"} aria-label={v.display_name ?? "Viewer"}>{initials(v.display_name)}</span>{/if}
    {/each}
    {#if ordered.length > p.max}<span class="tok more" role="img" aria-label={`${ordered.length - p.max} more`}>+{ordered.length - p.max}</span>{/if}
  </span>
  <span class="hub" aria-hidden="true"></span>
  <span class="side agt">
    {#each p.agents.slice(0, p.max) as a (a.handle)}
      {#if busy.has(a.handle)}<span class="tok a work" role="img" title={named(a)} aria-label={named(a)}>{mark(a)}</span>
      {:else if asleep(a)}<span class="tok a nowake" role="img" title={named(a) + NOWAKE} aria-label={named(a) + NOWAKE}>{mark(a)}</span>
      {:else if unsure(a)}<span class="tok a maywake" role="img" title={named(a) + MAYWAKE} aria-label={named(a) + MAYWAKE}>{mark(a)}</span>
      {:else}<span class="tok a" role="img" title={named(a)} aria-label={named(a)}>{mark(a)}</span>{/if}
    {/each}
    {#if p.agents.length > p.max}<span class="tok more" role="img" aria-label={`${p.agents.length - p.max} more agents`}>+{p.agents.length - p.max}</span>{/if}
  </span>
{/snippet}

{#if p.small}<span class="ros sm">{@render roster()}</span>{:else}<span class="ros">{@render roster()}</span>{/if}

<!-- The tokens' styles travel with this component (injected on mount): the
     gallery and the top bar load it, and the thread markers and the working
     strip reuse its tokens; so does the gallery's working chip. -->
<style>
  :global {
    .ros { display: flex; align-items: center; gap: 2px; }
    .ros .side { display: flex; gap: 2px; } .ros .ppl { flex-direction: row-reverse; }
    .ros:not(:has(.tok)) { display: none; }
    .ros .hub { width: 7px; height: 7px; border-radius: 50%; background: var(--fg); margin: 0 4px; flex: none; }
    .tok { display: inline-grid; place-items: center; height: 26px; min-width: 30px; padding: 0 6px; font: 600 11.5px/1 var(--font); border-radius: 13px; flex: none; position: relative; }
    .tok.p { background: var(--comment-hl); color: var(--you-ink); }
    .tok.p.away { opacity: .5; }
    .tok.p.here::before { content: ""; position: absolute; right: 2px; top: 2px; width: 5px; height: 5px; border-radius: 50%; background: var(--agent); box-shadow: 0 0 0 1.5px var(--comment-hl); }
    .ros.sm .tok.p.here::before, .tok.sm.p.here::before { width: 4px; height: 4px; }
    .tok.p.me::after { content: ""; position: absolute; left: 7px; right: 7px; bottom: 3px; height: 1.5px; background: currentColor; }
    .tok.a { background: var(--accent-tint); color: var(--agent-ink); }
    .tok.a.nowake { background: none; outline: 1.5px dashed var(--agent-ink); outline-offset: -1.5px; }
    .tok.a.maywake { outline: 1.5px dotted var(--agent-ink); outline-offset: -1.5px; }
    .tok.a.work { background: var(--agent); color: var(--on-accent); padding-left: 13px; }
    .tok.a.work::before { content: ""; position: absolute; left: 5px; top: 50%; width: 4px; height: 4px; margin-top: -2px; border-radius: 50%; background: currentColor; animation: breathe 1.8s ease-in-out infinite; }
    .tok.more { background: none; box-shadow: none; color: var(--muted); min-width: 0; padding: 0 3px; }
    .ros.sm .tok, .tok.sm { height: 20px; min-width: 24px; font-size: 11px; padding: 0 5px; }
    .ros.sm .tok.a.work { padding-left: 11px; } .ros.sm .tok.a.work::before { left: 4px; }
    @keyframes breathe { 50% { opacity: .3; } }
    @media (prefers-reduced-motion: reduce) { .tok.a.work::before { animation: none; } }
    .chip.ag { background: var(--accent-tint); color: var(--agent-ink); }
    .card .ft .ros { margin-right: auto; }
  }
</style>
