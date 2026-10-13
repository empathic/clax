<svelte:options css="injected" />

<script lang="ts">
  // The Send button's menu (spec 2026-10-08-multi-agent-multi-person §10),
  // loaded when the caret first opens it: the live agents by name, "Your
  // agents" first, then each other person's ("Sam's agents"), headed when
  // more than your own are listed; another person's agent names its
  // person, and says so when that person approves the threads' authors
  // first, as the daemon answers for this viewer (`GET
  // /api/artifacts/{aid}/approves-first`, the owner's and connected
  // people's browsers only). Escape closes it. Names are text.
  import type { AgentView } from "../view/working-model";
  import { sendGroups } from "../view/send-menu";

  let { agents, names, me, aid, threads, target, onChoose, onClose }: {
    agents: AgentView[]; names?: Map<string, string>; me: string | null | undefined; aid?: string; threads?: string[]; target: string | null; onChoose(h: string): void; onClose(): void;
  } = $props();
  let firsts = $state<(string | null)[]>([]);
  $effect(() => {
    if (!aid || !threads?.length) return;
    let live = true;
    void fetch(`/api/artifacts/${encodeURIComponent(aid)}/approves-first?threads=${threads.map(encodeURIComponent).join(",")}`)
      .then(r => (r.ok ? r.json() : null)).then(b => { if (live && Array.isArray(b?.approves_first)) firsts = b.approves_first; }, () => {});
    return () => { live = false; };
  });
  const groups = $derived(sendGroups(agents, me, firsts, names));
  const headed = $derived(groups.length > 1 || !groups[0]?.mine);
</script>

<!-- Escape closes the menu. -->
<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="send-menu" role="menu" tabindex="-1" onkeydown={e => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
  {#each groups as g (g.label)}
    <div role="group" aria-label={g.label}>
      {#if headed}<p class="send-group-h" aria-hidden="true">{g.label}</p>{/if}
      {#each g.agents as a (a.handle)}
        <button type="button" role="menuitemradio" aria-checked={a.handle === target} class="ghost" onclick={() => { onChoose(a.handle); onClose(); }}
          ><span class="send-name">{a.name}</span>{#if a.person}{" "}<small class="send-person">{a.person}</small>{/if}{#if a.first}{" "}<small class="send-first">{a.first}</small>{/if}</button>
      {/each}
    </div>
  {/each}
</div>

<style>
  :global {
    .send-menu { position: absolute; right: 0; top: calc(100% + 6px); z-index: 20; min-width: 180px; max-width: min(320px, calc(100vw - 32px)); background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); display: flex; flex-direction: column; padding: 4px 0; }
    .send-menu [role="group"] + [role="group"] { border-top: 1px solid var(--border); margin-top: 4px; padding-top: 4px; }
    .send-menu .send-group-h { margin: 4px 12px 2px; font: 600 11.5px var(--font); color: var(--muted); }
    .send-menu button { width: 100%; justify-content: flex-start; flex-wrap: wrap; gap: 0 8px; border: 0; border-radius: 0; min-height: 32px; text-align: start; }
    .send-menu button[aria-checked="true"]::before { content: "✓"; margin-right: -2px; }
    .send-menu .send-name { min-width: 0; overflow-wrap: anywhere; }
    .send-menu .send-first, .send-menu .send-person { font: 400 11.5px var(--font); color: var(--muted); }
    .send-menu .send-person + .send-first::before { content: "· "; }
    @media (max-width: 700px) { .send-menu button { min-height: 40px; } }
  }
</style>
