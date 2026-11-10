<script lang="ts">
  // The version control (spec §8). On the latest version, with nothing newer
  // out, it is one menu button, `Latest ▾`. Otherwise it is a split button:
  // its main part goes to the latest version, reading `Reload for latest`
  // once a version newer than the page's has been published (`newer`), and
  // `v12 of 46` on an older version the viewer chose; its ▾ part opens the
  // version panel. A green dot on the menu button marks a version newer than
  // this viewer's last view. On a live page the versions are snapshots. The
  // panel loads on first open.
  // At phone width the menu button and `v12 of 46` are hidden (the more
  // menu's Versions item opens the panel, as a sheet); `Reload for latest`
  // stays.
  import type { RowInput } from "../view/version-rows";
  import type VersionPanelT from "./VersionPanel.svelte";

  let { shown, latest, newer = null, dot, open, onToggle, onLatest, input, hrefFor, onChoose, live }: {
    shown: number; latest: number; dot: boolean; open: boolean; onToggle(): void; input: () => RowInput; hrefFor(n: number): string; onChoose(n: number): void;
    /** The newest version the event stream announced past `latest`, if any. */
    newer?: number | null;
    /** Loads the latest version; absent, the panel is the only way there. */
    onLatest?: () => void;
    /** A live page, whose versions are snapshots. */
    live?: boolean;
  } = $props();
  const noun = $derived(live ? "snapshot" : "version");
  const top = $derived(Math.max(latest, newer ?? 0));
  /** What the main part does: nothing (one menu button), back to the latest from an older pick, or reload for a newer version. */
  const action = $derived(!onLatest ? null : shown < latest ? "back" : newer !== null && newer > latest ? "reload" : null);
  const dotNote = $derived(dot ? `, a newer ${noun} you have not seen` : "");
  let button: HTMLButtonElement | undefined = $state();
  // The panel's code loads on first open (no await block: its runtime would
  // join the artifact entry).
  let Panel: typeof VersionPanelT | null = $state(null);
  $effect(() => { if (open && !Panel) void import("./VersionPanel.svelte").then(m => { Panel = m.default; }, () => {}); });
</script>

<div class={["version-menu", action && "split"]}>
  {#if action === "reload"}
    <button type="button" class="vmain primary reload" aria-label={`Reload for latest, ${noun} ${top}`} title={`Load ${noun} ${top}, published since this page loaded`} onclick={() => onLatest?.()}>Reload for latest</button>
  {:else if action === "back"}
    <button type="button" class="vmain hide-sm" aria-label={`v${shown} of ${top}, back to the latest`} title={`Go to the latest ${noun}, v${top}`} onclick={() => onLatest?.()}>v{shown}<span>of {top}</span></button>
  {/if}
  <button type="button" class="vbtn hide-sm" data-shell-control="versions" bind:this={button} aria-haspopup="dialog" aria-expanded={open}
    aria-label={action ? `${live ? "Snapshots" : "Versions"}${dotNote}` : shown < latest ? `v${shown} of ${latest}${dotNote}` : `Latest, ${noun} ${shown}${dotNote}`}
    title={action ? `All ${noun}s` : undefined} onclick={onToggle}>
    {#if action}<span aria-hidden="true">▾</span>{:else if shown < latest}v{shown}<span>of {latest} ▾</span>{:else}Latest<span>▾</span>{/if}{#if dot}<i class="new" aria-hidden="true"></i>{/if}
  </button>
  {#if open && Panel}
    <Panel input={input()} {hrefFor} {onChoose} onClose={() => { onToggle(); button?.focus(); }} />
  {/if}
</div>
