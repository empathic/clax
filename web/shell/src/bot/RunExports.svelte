<svelte:options css="injected" />

<script lang="ts">
  // Exporting a page of a run's state to one of the person's machine grants
  // (spec 2026-11-19-claxbot-browser §18.5, option A): the person, never the
  // model, picks a page the run's last committed state holds, a grant and
  // what it may do. The export is a grant row, listed and revoked here; its
  // capability is not released to the grant's holder in this version.
  import { refusal } from "../people/refusal";
  import { type RunExports, deleteRunExport, getRunExports, postRunExport } from "./api";

  let { run }: { run: string } = $props();
  let data = $state<RunExports | null>(null);
  let page = $state("");
  let grant = $state("");
  let readOnly = $state(true);
  let err = $state<string | null>(null);
  let busy = $state(false);

  function take(v: RunExports) {
    data = v;
    page = page || v.pages[0]?.path || "";
    grant = grant || v.grants[0]?.id || "";
  }
  async function act(fn: () => Promise<RunExports>) {
    busy = true; err = null;
    try { take(await fn()); } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  function opened(e: Event) {
    if ((e.currentTarget as HTMLDetailsElement).open && !data) void act(() => getRunExports(run));
  }
  const grantName = (id: string | null) => data?.grants.find(g => g.id === id)?.name ?? id ?? "";
</script>

<details class="rexports" ontoggle={opened}>
  <summary>Export a page to one of your agents</summary>
  {#if data}
    {#if data.pages.length && data.grants.length}
      <div class="rexform">
        <label>Page <select bind:value={page}>{#each data.pages as p (p.path)}<option value={p.path}>{p.path}</option>{/each}</select></label>
        <label>To <select bind:value={grant}>{#each data.grants as g (g.id)}<option value={g.id}>{g.name}</option>{/each}</select></label>
        <label><input type="checkbox" bind:checked={readOnly} /> Read only</label>
        <button type="button" aria-disabled={busy} onclick={() => { if (!busy) void act(() => postRunExport(run, { path: page, grant, actions: readOnly ? ["read"] : undefined })); }}>Export</button>
      </div>
    {:else}
      <p class="note">{data.pages.length ? "You have no connected agent to export to." : "The run's state holds no page."}</p>
    {/if}
    {#if data.exports.length}
      <ul class="capgrants" aria-label="Your exported pages">
        {#each data.exports as x (x.id)}
          <li><span>{grantName(x.grant)}: {x.actions.join(", ")}</span><button type="button" aria-label={`Revoke the page exported to ${grantName(x.grant)}`} onclick={() => void act(() => deleteRunExport(run, x.id))}>Revoke</button></li>
        {/each}
      </ul>
    {/if}
    {#if !data.released}<p class="note">An exported page is listed and revocable; its holder cannot use it yet.</p>{/if}
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</details>

<style>
  :global {
    .rexports { margin: 8px 0; }
    .rexform { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  }
</style>
