<svelte:options css="injected" />

<script lang="ts">
  // "Raise on this artifact" on one agent's row of the artifact's People
  // panel (spec 2026-10-08-multi-agent-multi-person §7.4, §17, O11): for
  // the artifact's creator, the actions they may give there (`holds`), each
  // a checkbox that sets the creator's own elevation of that agent, at this
  // artifact, keeping what it raises elsewhere (`mine`). A raise lasts until
  // the agent's session ends; the agent's person sees who gave it. Loaded
  // in its own chunk, only for a creator; a refusal shows as the daemon
  // words it.
  import type { Rights } from "../api";
  import { creatorCaps, elevation } from "../people/model";

  type Props = { aid: string; handle: string; name: string; holds: Rights; mine: Rights | null; onChanged(): void };
  let { aid, handle, name, holds, mine, onChanged }: Props = $props();
  let busy = $state(false);
  let err = $state<string | null>(null);
  const caps = $derived(creatorCaps(holds, mine, aid));
  const raised = $derived(caps.some(c => c.on));

  async function change(e: Event, key: string) {
    const el = e.currentTarget as HTMLInputElement;
    if (busy) { el.checked = !el.checked; return; }
    busy = true; err = null;
    const access = elevation({ elevate: mine }, key, el.checked, aid);
    const path = `/api/agents/${encodeURIComponent(handle)}/elevate`;
    try {
      const res = await fetch(path, access ? { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ access }) } : { method: "DELETE" });
      if (!res.ok) {
        let b: { error?: { message?: string } } = {};
        try { b = await res.json(); } catch { /* not json */ }
        throw new Error(b.error?.message ?? `HTTP ${res.status}`);
      }
      onChanged();
    } catch (x) {
      err = x instanceof Error ? x.message : String(x);
      el.checked = !el.checked;
    } finally { busy = false; }
  }
</script>

<details class="raise-here" aria-busy={busy}>
  <summary>Raise on this artifact{raised ? " (raised)" : ""}</summary>
  <p class="rnote">What {name} may also do here, until its session ends. You give only what you may do here yourself.</p>
  <div class="rcaps">
    {#each caps as c (c.key)}
      <label><input type="checkbox" checked={c.on} onchange={e => change(e, c.key)} /> {c.label}</label>
    {/each}
  </div>
  {#if err}<p class="rerr" role="alert">{err}</p>{/if}
</details>

<style>
  :global {
    .raise-here { grid-column: 2; margin-top: 4px; font-size: 12.5px; }
    .raise-here summary { cursor: pointer; color: var(--accent-ink); font-weight: 600; min-height: 28px; display: flex; align-items: center; }
    .raise-here .rnote { margin: 2px 0 6px; color: var(--muted); line-height: 1.45; }
    .raise-here .rcaps { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 0 12px; }
    .raise-here label { display: flex; align-items: center; gap: 6px; color: var(--fg); min-height: 28px; }
    .raise-here .rerr { margin: 6px 0 0; color: var(--danger); }
  }
</style>
