<script lang="ts">
  // The artifact view's closed sidebar: the strip (Rail.svelte) on the edge
  // the drawer docks to, wired to the controller; activating it shows the
  // threads. The timeline it draws is the sidebar's (timeline-model.ts),
  // over the same threads, versions, agents at work and ClaxBot's threads.
  // It comes in the sidebar's chunk, after the first paint (`loadSidebar`,
  // sidebar-chunk.ts); the sidebar island holds its box until then.
  // The plot's height, which says how many lines fit, is measured here.
  import { botView } from "../bot/send.svelte";
  import type { ArtifactController, Loaded, ViewState } from "../view/artifact-controller";
  import { timeline } from "../view/timeline-model";
  import { drawer } from "./drawer.svelte";
  import Rail from "./Rail.svelte";

  let { ctl, s }: { ctl: ArtifactController; s: ViewState & { data: Loaded } } = $props();
  // Each input on its own, so the timeline is computed again only when one
  // of them is a new value, not on every change of the controller's state.
  const threads = $derived(s.threads);
  const versions = $derived(s.data.versions);
  const working = $derived(s.working);
  const claimed = $derived(botView.claimed);
  const me = $derived(s.me);
  const tl = $derived(timeline({ threads, versions, working, claimed, me }));
  let height = $state(0);
  let box: HTMLElement | undefined;
  // Showing the threads puts the drawer in the strip's place, taking the
  // strip's button away: when it held focus, focus goes on to the drawer's
  // Hide threads, which closes it again (as Hide threads hands it to the top
  // bar's Threads, DrawerChrome.svelte), once the drawer is in the island.
  // It stops looking once the drawer is there, or the strip is back without
  // it, and at the latest once the strip has gone: the drawer takes its
  // place in the same update, so a last look follows that update, and none
  // after it.
  let waiting: MutationObserver | null = null;
  const open = () => {
    const island = box?.parentElement;
    const held = !!box?.contains(document.activeElement);
    ctl.togglePanel();
    if (!held || !island) return;
    const go = () => { const b = island.querySelector<HTMLElement>('.drawer button[aria-label="Hide threads"]'); b?.focus(); return !!b; };
    if (go()) return;
    waiting?.disconnect();
    const mo = new MutationObserver(() => { if (go() || island.querySelector("button.rail")) mo.disconnect(); });
    mo.observe(island, { childList: true, subtree: true });
    waiting = mo;
  };
  $effect(() => () => {
    const mo = waiting;
    waiting = null;
    // Its records of this update reach it first, in their own microtask.
    if (mo) queueMicrotask(() => mo.disconnect());
  });
  const plot = (el: HTMLElement) => {
    box = el;
    const at = el.querySelector<HTMLElement>(".rail-plot");
    if (!at) return;
    const on = () => { height = at.clientHeight; };
    on();
    if (typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver(on);
    ro.observe(at);
    return () => ro.disconnect();
  };
</script>

<aside class="rail" aria-label="Comment threads" {@attach plot}>
  <Rail {tl} side={drawer.prefs.layout === "left" ? "left" : "right"} working={s.working.length > 0} {height} onOpen={open} />
</aside>
