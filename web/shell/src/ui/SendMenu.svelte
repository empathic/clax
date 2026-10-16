<svelte:options css="injected" />

<script lang="ts">
  // The Send button's menu (spec 2026-10-08-multi-agent-multi-person §10),
  // loaded when the caret first opens it: the live agents by name, "Your
  // agents" first, then each other person's ("Sam's agents"), headed when
  // more than your own are listed; another person's agent names its
  // person, and says so when that person approves the threads' authors
  // first, as the daemon answers for this viewer (`GET
  // /api/artifacts/{aid}/approves-first`, the owner's and connected
  // people's browsers only; a host may ask its own way, `firstsVia`).
  // It takes the keyboard's focus as it opens (the checked agent, else the
  // first); the arrow keys, Home and End move through it; Escape, there or
  // on the button that opened it, closes it and gives focus back to that
  // button, as choosing an agent does; Tab out of it closes it. Names are
  // text.
  import type { AgentView } from "../view/working-model";
  import { type AskFirsts, firstsVia, sendGroups } from "../view/send-menu";

  let { agents, names, me, own = me, aid, threads, target, onChoose, onClose }: {
    agents: AgentView[]; names?: Map<string, string>; me: string | null | undefined; own?: string | null; aid?: string; threads?: string[]; target: string | null; onChoose(h: string): void; onClose(): void;
  } = $props();
  const ask: AskFirsts = firstsVia.ask ?? ((a, ts) =>
    fetch(`/api/artifacts/${encodeURIComponent(a)}/approves-first?threads=${ts.map(encodeURIComponent).join(",")}`)
      .then(r => (r.ok ? r.json() : null)).then(b => (Array.isArray(b?.approves_first) ? b.approves_first : null)));
  let firsts = $state<(string | null)[]>([]);
  $effect(() => {
    if (!aid || !threads?.length) return;
    let live = true;
    void ask(aid, threads).then(f => { if (live && f) firsts = f; }, () => {});
    return () => { live = false; };
  });
  const groups = $derived(sendGroups(agents, me, firsts, names));
  // Unheaded, and its agents untagged, when it lists only the viewer's own
  // agents, or only those a Send takes as its own (`own`: a LAN viewer's,
  // the owner's), as on a single-owner install.
  const pool = $derived(groups.length === 1 && agents.every(a => !a.live || !a.person || a.person.public_id === own));
  const headed = $derived(groups.length > 1 || !(groups[0]?.mine || pool));
  let menu: HTMLElement | undefined = $state();
  // The button that opened it, which gets the focus back.
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [])];
  $effect(() => { const all = items(); (all.find(i => i.getAttribute("aria-checked") === "true") ?? all[0])?.focus(); });
  function close() { onClose(); opener?.focus(); }
  function key(e: KeyboardEvent) {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    const to = e.key === "ArrowDown" ? (at + 1) % all.length : e.key === "ArrowUp" ? (at < 0 ? all.length - 1 : (at - 1 + all.length) % all.length) : e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : -1;
    if (to >= 0 && all.length) { e.preventDefault(); all[to].focus(); }
    else if (e.key === "Escape") { e.stopPropagation(); close(); }
  }
  // Tab out of it closes it, focus going where Tab took it.
  const out = (e: FocusEvent) => { const to = e.relatedTarget as Node | null; if (to && !menu?.parentElement?.contains(to)) onClose(); };
  // Escape with the focus on the button beside it (its caret, or a Send that opened it).
  const away = (e: KeyboardEvent) => { if (e.key === "Escape" && menu?.parentElement?.contains(document.activeElement)) { e.stopPropagation(); close(); } };
</script>

<svelte:window onkeydown={away} />

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="send-menu" role="menu" tabindex="-1" bind:this={menu} onkeydown={key} onfocusout={out}>
  {#each groups as g (g.label)}
    <div role="group" aria-label={g.label}>
      {#if headed}<p class="send-group-h" aria-hidden="true">{g.label}</p>{/if}
      {#each g.agents as a (a.handle)}
        <button type="button" role="menuitemradio" aria-checked={a.handle === target} class="ghost" tabindex="-1" onclick={() => { onChoose(a.handle); close(); }}
          ><span class="send-name">{a.name}</span>{#if a.person && headed}{" "}<small class="send-person">{a.person}</small>{/if}{#if a.first}{" "}<small class="send-first">{a.first}</small>{/if}</button>
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
