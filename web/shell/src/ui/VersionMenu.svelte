<script lang="ts">
  // The version button (spec §8): `v5 of 5`, a green dot while a version
  // newer than this viewer's last view exists. Its panel loads on first open.
  // At phone width the button is hidden and the more menu's Versions item
  // opens the panel, as a sheet.
  import type { RowInput } from "../view/version-rows";
  import type VersionPanelT from "./VersionPanel.svelte";

  let { shown, latest, dot, open, onToggle, input, hrefFor, onChoose }: {
    shown: number; latest: number; dot: boolean; open: boolean; onToggle(): void; input: () => RowInput; hrefFor(n: number): string; onChoose(n: number): void;
  } = $props();
  let button: HTMLButtonElement | undefined = $state();
  // The panel's code loads on first open (no await block: its runtime would
  // join the artifact entry).
  let Panel: typeof VersionPanelT | null = $state(null);
  $effect(() => { if (open && !Panel) void import("./VersionPanel.svelte").then(m => { Panel = m.default; }, () => {}); });
</script>

<div class="version-menu">
  <button type="button" class="vbtn hide-sm" bind:this={button} aria-haspopup="dialog" aria-expanded={open} aria-label={`Version ${shown} of ${latest}${dot ? ", a newer version you have not seen" : ""}`} onclick={onToggle}>
    v{shown}<span>of {latest} ▾</span>{#if dot}<i class="new" aria-hidden="true"></i>{/if}
  </button>
  {#if open && Panel}
    <Panel input={input()} {hrefFor} {onChoose} onClose={() => { onToggle(); button?.focus(); }} />
  {/if}
</div>
