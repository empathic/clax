<svelte:options css="injected" />

<script lang="ts">
  // "Raise on this artifact" on one agent's row of the artifact's People
  // panel (spec 2026-10-08-multi-agent-multi-person §7.4, §17, O11): for
  // the artifact's creator, the actions they may give there (`holds`), each
  // a checkbox that sets the creator's own elevation of that agent, at this
  // artifact, keeping what it raises elsewhere (`mine`). Each change names
  // the raise it was built on (`version`, null for none), so a panel opened
  // before the agent's person or the owner removed the raise never brings it
  // back: the daemon answers 409 and the panel shows the raise as it
  // stands. Opening it refreshes it. A raise lasts until the agent's session
  // ends or its person or the owner removes it, and the agent's narrowing
  // caps it; its person sees who gave it. Loaded in its own chunk, only for
  // a creator; another refusal shows as the daemon words it.
  import type { Rights } from "../api";
  import { creatorCaps, elevation } from "../people/model";

  type Props = { aid: string; handle: string; name: string; holds: Rights; mine: Rights | null; version: string | null; onChanged(): void };
  let { aid, handle, name, holds, mine, version, onChanged }: Props = $props();
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
      const res = await fetch(path, access ? { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ access, if_version: version }) } : { method: "DELETE" });
      if (!res.ok) {
        let b: { error?: { code?: string; message?: string } } = {};
        try { b = await res.json(); } catch { /* not json */ }
        // Changed since this panel was built: show it as it stands now.
        if (b.error?.code === "conflict" || b.error?.code === "not_raised") {
          el.checked = !el.checked;
          err = "This raise changed since you opened it (its agent's person or the owner may have removed it). It is shown as it stands now.";
          onChanged();
          return;
        }
        throw new Error(b.error?.message ?? `HTTP ${res.status}`);
      }
      onChanged();
    } catch (x) {
      err = x instanceof Error ? x.message : String(x);
      el.checked = !el.checked;
    } finally { busy = false; }
  }
</script>

<details class="raise-here" aria-busy={busy} ontoggle={e => { if (e.currentTarget.open) onChanged(); }}>
  <summary>Raise on this artifact{raised ? " (raised)" : ""}</summary>
  <p class="rnote">What {name} may also do here, until its session ends or its person or the owner removes it; its person's narrowing caps it. You give only what you may do here yourself.</p>
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
    /* A list item, as summaries are, so its disclosure marker shows. */
    .raise-here summary { cursor: pointer; color: var(--accent-ink); font-weight: 600; display: list-item; line-height: 1.4; padding: 4px 0; }
    .raise-here .rnote { margin: 2px 0 6px; color: var(--muted); line-height: 1.45; }
    .raise-here .rcaps { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 0 12px; }
    .raise-here label { display: flex; align-items: center; gap: 6px; color: var(--fg); min-height: 28px; }
    .raise-here .rerr { margin: 6px 0 0; color: var(--danger); }
  }
</style>
