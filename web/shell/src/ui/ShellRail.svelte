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
  const tl = $derived(timeline({ threads: s.threads, versions: s.data.versions, working: s.working, claimed: botView.claimed, me: s.me }));
  let height = $state(0);
  const plot = (el: HTMLElement) => {
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
  <Rail {tl} side={drawer.prefs.layout === "left" ? "left" : "right"} working={s.working.length > 0} {height} onOpen={() => ctl.togglePanel()} />
</aside>
