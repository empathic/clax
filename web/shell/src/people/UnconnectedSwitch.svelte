<svelte:options css="injected" />

<script lang="ts">
  import { refusal } from "./refusal";
  // The `*unconnected` switch (spec 2026-10-08-multi-agent-multi-person
  // §9.6), the owner's: on while the owner's approval of viewers who are not
  // connected is live. Off revokes that approval; on makes a new one. A
  // refusal is shown as the daemon words it, and the switch stays as it was.
  // A flip while one is in flight is ignored; the switch stays enabled so
  // the keyboard's focus stays on it. Its name is short; the §9.6 text
  // describes it.
  import * as api from "./api";

  let { sw, onChanged }: { sw: { on: boolean; approval_id: string | null }; onChanged(): void } = $props();
  let busy = $state(false);
  let err = $state<string | null>(null);
  const flip = async () => {
    if (busy) return;
    busy = true; err = null;
    try {
      if (sw.on && sw.approval_id) await api.revokeApproval(sw.approval_id);
      else await api.allow("*unconnected");
      onChanged();
    } catch (e) { err = refusal(e); } finally { busy = false; }
  };
</script>

<div class="unconn">
  <button type="button" role="switch" class="sw" aria-checked={sw.on} aria-label="Comments from people who are not connected reach your agents" aria-describedby="unconn-l" aria-busy={busy} onclick={flip}><span></span></button>
  <p id="unconn-l">Comments from people on your LAN who are not connected reach your agents without asking. This includes connected people commenting from a browser or script that is not signed in as them.</p>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</div>

<style>
  :global {
    .unconn { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; align-items: start; }
    .unconn p { margin: 0; font-size: 13.5px; line-height: 1.45; }
    .unconn .err { grid-column: 2; }
    .sw { width: 40px; min-height: 24px; height: 24px; padding: 2px; border-radius: 999px; justify-content: flex-start; background: var(--border-hover); border-color: transparent; }
    .sw span { width: 18px; height: 18px; border-radius: 50%; background: var(--card); box-shadow: var(--elev); transition: transform var(--t); }
    .sw[aria-checked="true"] { background: var(--accent); }
    .sw[aria-checked="true"] span { transform: translateX(16px); }
    .sw:not(:disabled):hover { background-color: var(--border-strong); border-color: transparent; }
    .sw[aria-checked="true"]:not(:disabled):hover { background: var(--accent-hover); }
  }
</style>
