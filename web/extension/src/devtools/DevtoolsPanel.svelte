<script lang="ts">
  // The Clax DevTools panel (RECOMMEND-consultant-2026-11-02-clax-extension-
  // devtools Q1): a third host of the side panel's view, pinned to the
  // inspected tab. Five views in a tab list (arrows, Home and End move
  // between them): Threads (the side panel's own `Panel`, unchanged), Queues
  // (Clax's own queue and report pages, pinned to the inspected site),
  // Integration (how Clax is installed on the page, every config field and
  // the viewer's override, with no host code), App
  // settings (the page's dev settings hook) and Network. One notices zone
  // under the tab list, always there and of a fixed height, says what each
  // change did (a live region), so nothing below it moves. Every view stays
  // mounted while hidden once opened, so switching keeps its state.
  import type { ComponentProps } from "svelte";
  import Panel from "../panel/Panel.svelte";
  import AppSettings from "./AppSettings.svelte";
  import IntegrationView from "./IntegrationView.svelte";
  import type { IntegrationLink } from "./integration.svelte";
  import NetworkView from "./NetworkView.svelte";
  import QueuesView from "./QueuesView.svelte";
  import type { NetworkLog } from "./network.svelte";
  import type { SettingsLink } from "./settings.svelte";

  type View = "threads" | "queues" | "integration" | "settings" | "network";
  const VIEWS: { id: View; label: string }[] = [
    { id: "threads", label: "Threads" }, { id: "queues", label: "Queues" }, { id: "integration", label: "Integration" }, { id: "settings", label: "App settings" }, { id: "network", label: "Network" },
  ];
  let { link, queues, inspected = { origin: null, page: null }, show = async () => false, settings, integration, network, shown = true, reload, copy = copyText, now }: {
    link: ComponentProps<typeof Panel>["link"];
    /** The owner's queues, through the worker. */
    queues: ComponentProps<typeof QueuesView>["api"];
    /** The inspected page's origin and address. */
    inspected?: { origin: string | null; page: string | null };
    /** Shows a selector's element in the inspected page; true when found. */
    show?(selector: string): Promise<boolean>;
    settings: Pick<SettingsLink, "status" | "snapshot" | "set" | "reset" | "active">;
    /** The page's Clax install, read through `inspectedWindow.eval`. */
    integration: Pick<IntegrationLink, "status" | "info" | "set" | "refresh">;
    network: Pick<NetworkLog, "rows" | "keep" | "clear">;
    /** Whether DevTools shows this panel now (its `onShown` / `onHidden`). */
    shown?: boolean;
    /** Reloads the inspected page. */
    reload(): void;
    copy?(text: string): Promise<boolean>;
    now?: Date;
  } = $props();
  let view = $state<View>("threads");
  /** The views opened once, which stay mounted. */
  let seen = $state<Partial<Record<View, true>>>({ threads: true });
  $effect(() => { if (!seen[view]) seen = { ...seen, [view]: true }; });
  let notice = $state<{ text: string; reload: boolean; n: number } | null>(null);
  let said = 0;
  const say = (text: string, withReload = false) => { notice = { text, reload: withReload, n: ++said }; };
  // The page's settings are read only while they are in sight.
  $effect(() => { settings.active(shown && view === "settings"); });
  // The install is read again each time the view comes into sight.
  $effect(() => { if (shown && view === "integration") void integration.refresh(); });
  function key(e: KeyboardEvent): void {
    const i = VIEWS.findIndex(v => v.id === view);
    const to = e.key === "ArrowRight" ? (i + 1) % VIEWS.length : e.key === "ArrowLeft" ? (i + VIEWS.length - 1) % VIEWS.length : e.key === "Home" ? 0 : e.key === "End" ? VIEWS.length - 1 : -1;
    if (to < 0) return;
    e.preventDefault();
    view = VIEWS[to].id;
    queueMicrotask(() => document.getElementById(`dt-${view}`)?.focus());
  }
  /** Copies `text`; DevTools' panel frame may refuse the async clipboard, so a hidden field's copy is the fallback. */
  async function copyText(text: string): Promise<boolean> {
    try { await navigator.clipboard.writeText(text); return true; } catch { /* fall back */ }
    const t = document.createElement("textarea");
    t.value = text;
    t.setAttribute("readonly", "");
    t.style.position = "fixed";
    t.style.opacity = "0";
    document.body.append(t);
    t.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch { /* refused */ }
    t.remove();
    return ok;
  }
</script>

<div class="host">
  <div class="tabs" role="tablist" aria-label="Clax">
    {#each VIEWS as v (v.id)}
      <button type="button" role="tab" id="dt-{v.id}" aria-selected={view === v.id} aria-controls="dt-{v.id}-panel" tabindex={view === v.id ? 0 : -1}
        onclick={() => (view = v.id)} onkeydown={key}>{v.label}</button>
    {/each}
  </div>
  <!-- The notices zone: reserved, one line, whether or not anything is said. -->
  <div class="notices">
    <p role="status" aria-live="polite" title={notice?.text}>{notice?.text ?? ""}</p>
    {#if notice?.reload}
      <button type="button" class="ghost" onclick={() => { reload(); say("Reloading the page."); }}>Reload page</button>
    {/if}
  </div>
  <div class="view" role="tabpanel" id="dt-threads-panel" aria-labelledby="dt-threads" hidden={view !== "threads"}>
    <Panel {link} {now} />
  </div>
  <div class="view" role="tabpanel" id="dt-queues-panel" aria-labelledby="dt-queues" hidden={view !== "queues"}>
    <!-- Loaded once first opened, then kept: the queue routes are read only for a panel that shows them. -->
    {#if seen.queues}<QueuesView api={queues} origin={inspected.origin} page={inspected.page} {show} {say} />{/if}
  </div>
  <div class="view" role="tabpanel" id="dt-integration-panel" aria-labelledby="dt-integration" hidden={view !== "integration"}>
    <IntegrationView link={integration} {say} />
  </div>
  <div class="view" role="tabpanel" id="dt-settings-panel" aria-labelledby="dt-settings" hidden={view !== "settings"}>
    <AppSettings link={settings} {say} />
  </div>
  <div class="view fill" role="tabpanel" id="dt-network-panel" aria-labelledby="dt-network" hidden={view !== "network"}>
    <NetworkView log={network} {say} {copy} />
  </div>
</div>

<style>
  :global(body) { margin: 0; background: var(--bg); color: var(--fg); }
  .host { display: grid; grid-template-rows: auto 32px minmax(0, 1fr); height: 100vh; background: var(--bg); color: var(--fg); font: 14px/1.45 var(--font); }
  .tabs { display: flex; gap: 2px; padding: 0 var(--gutter); border-bottom: 1px solid var(--border); background: var(--card); }
  .tabs button { flex: none; min-height: 32px; padding: 0 10px; border: 0; border-bottom: 2px solid transparent; border-radius: 0; background: none; color: var(--muted); font-weight: 500; }
  .tabs button[aria-selected="true"] { color: var(--fg); border-bottom-color: var(--fg); }
  .tabs button:not(:disabled):hover { background: var(--hover); }
  .notices { display: flex; align-items: center; gap: 8px; padding: 0 var(--gutter); border-bottom: 1px solid var(--border); overflow: hidden; }
  .notices p { flex: 1; margin: 0; font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .notices button { min-height: 24px; font-size: 12.5px; }
  .view { overflow: auto; min-height: 0; }
  .view.fill { overflow: hidden; }
  .view[hidden] { display: none; }
  /* The side panel fills its own window; here it is one view of the host. */
  .view :global(.panel) { min-height: 100%; }
</style>
