<script lang="ts">
  // Moving a thread to another page of the site (owner decision 2026-10-06):
  // a page the site's listing has, or the tab's page. The worker checks the
  // page is of the tab's origin; the daemon makes it when it is missing.
  type Target = { url: string; label: string };
  let { targets, onMove, onCancel }: { targets: Target[]; onMove(url: string): void; onCancel(): void } = $props();
  let url = $state("");
  $effect(() => { if (!targets.some(t => t.url === url)) url = targets[0]?.url ?? ""; });
</script>

<div class="move-to" role="group" aria-label="Move thread">
  <label><span>Move to</span>
    <select aria-label="Move to page" value={url} onchange={e => (url = e.currentTarget.value)}>
      {#each targets as t (t.url)}<option value={t.url}>{t.label}</option>{/each}
    </select>
  </label>
  <div class="row">
    <button type="button" class="primary" disabled={!url} onclick={() => onMove(url)}>Move</button>
    <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
  </div>
</div>

<style>
  .move-to { display: grid; gap: 8px; margin-top: 8px; padding: 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); }
  label { display: grid; gap: 4px; font-size: 12.5px; color: var(--muted); min-width: 0; }
  select { width: 100%; min-width: 0; font: 13px var(--font); padding: 6px 8px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--card); color: var(--fg); text-overflow: ellipsis; }
  .row { display: flex; gap: 6px; justify-content: flex-end; }
</style>
