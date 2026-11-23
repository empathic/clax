<script lang="ts">
  // The sidebar island: the comment threads, once the artifact is loaded and
  // the frame mode decided; then the phone's Page | Threads switch, after
  // them so the Tab order follows the screen. The thread list's code and
  // styles, and the controller's wiring of it (ShellSidebar), load after the
  // first paint (`loadSidebar`); until then an empty sidebar holds its
  // place, so the frame is laid out once at its final size. Wider than a
  // phone the sidebar sits in the drawer, docked or floating, at the place
  // and size this browser remembers (view/drawer-model.ts), which the
  // page's head script put on the root; its controls, and the floating
  // window's box, come with the thread list's chunk. Closed while docked
  // left or right wider than a phone, the sidebar leaves a strip on that
  // edge (`railShows`), whose code (ShellRail) comes in the same chunk;
  // until then an empty box of its size holds its place. Where the threads
  // open, the list and the page's pins show together, once the list is
  // drawn with its questions above it (`view/deciders.svelte.ts`) and, for
  // the owner or a connected person with a thread sent to an agent, the
  // approval cards above those threads (asked as the chunk comes, waited
  // for at most `APPROVALS_WAIT_MS`, ui/thread-approvals.ts): until
  // then the page is `.cards-wait`, and the threads' skeleton
  // (`view/drawer-shape.ts`) holds the list's place; once the list shows,
  // its shape is kept for the next load's skeleton, and once the view is
  // ready, whether the artifact has a thread (the next load's strip), both
  // by the thread list's chunk.
  import { fromStore } from "svelte/store";
  import { inspectQuery } from "../route";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { afterPaint } from "../view/after-paint";
  import { decided } from "../view/deciders.svelte";
  import PhoneTabs from "./PhoneTabs.svelte";
  import type RailT from "./ShellRail.svelte";
  import type SidebarT from "./ShellSidebar.svelte";
  import { loadSidebar, railShows, sidebarPrefetch } from "./sidebar-chunk";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  // Wider than a phone, followed as the window changes.
  const media = typeof matchMedia === "function" ? matchMedia("(min-width: 701px)") : null;
  let wide = $state(!!media?.matches);
  $effect(() => {
    const on = () => { wide = !!media?.matches; };
    media?.addEventListener?.("change", on);
    return () => media?.removeEventListener?.("change", on);
  });
  const inspecting = inspectQuery(location.search);
  const rail = $derived(viewReady(s) && railShows(s, document.documentElement.dataset.drawer, wide, inspecting));
  let Sidebar: typeof SidebarT | null = $state(null);
  let Rail: typeof RailT | null = $state(null);
  let wait: Pick<typeof import("./ShellSidebar.svelte"), "keepShape" | "listReady"> | null = $state(null);
  let asked = false;
  let cancel: (() => void) | undefined;
  const take = () => {
    loadSidebar().then(m => { Sidebar = m.default; Rail = m.ShellRail; wait = m; }, () => ctl.setNotice("The threads could not load. Reload to try again."));
  };
  $effect(() => {
    if (asked || !(s.panel || rail || (sidebarPrefetch() && !s.deleted))) return;
    // Where the threads open (`s.panel`), the chunk is asked at once, beside
    // the view's data, so the list is drawn with the view; else after the
    // first paint, once the view is ready.
    if (s.panel && !viewReady(s)) { asked = true; take(); return; }
    if (!viewReady(s)) return;
    asked = true;
    cancel = afterPaint(take);
  });
  $effect(() => () => cancel?.());
  // Whether the list may show (its approval cards in: the chunk's `listReady`), asked once the threads open.
  let ready = $state(0);
  $effect(() => { if (!ready && s.panel && viewReady(s) && wait) { ready = 1; void wait.listReady(ctl.id, s).then(() => { ready = 2; }); } });
  const waiting = $derived(viewReady(s) && s.panel && !(Sidebar && decided.questions && ready > 1));
  $effect(() => { document.querySelector(".page")?.classList.toggle("cards-wait", waiting); });
  // The shape kept for the next load: once the view is ready, and again with the list's blocks once it shows.
  let kept = 0;
  let unkeep: (() => void) | undefined;
  $effect(() => {
    const list = !waiting && s.panel;
    if (kept > (list ? 1 : 0) || !viewReady(s) || s.deleted || !wait) return;
    kept = list ? 2 : 1;
    const m = wait;
    unkeep?.();
    const n = s.threads.length;
    unkeep = afterPaint(() => m.keepShape(ctl.id, n, list ? document.querySelector(".viewer .drawer") : null));
  });
  $effect(() => () => unkeep?.());
  // The static frame's drawer box and strip (`.drawer.held`, `.rail-held`: skeleton.ts) give way to
  // this island's own in the render that draws it.
  $effect.pre(() => {
    if (viewReady(s)) document.querySelectorAll(".viewer > :is(.drawer.held, .rail-held)").forEach(e => e.remove());
  });
</script>

{#if viewReady(s)}
  <!-- Until its controls load, the drawer is this empty sidebar; a floating
       drawer, which takes no room from the page, is not drawn. -->
  {#if s.panel && !Sidebar && document.documentElement.dataset.drawer !== "float"}
    <aside class="sidebar drawer" aria-label="Comment threads" aria-busy="true"></aside>
  {:else if s.panel && Sidebar}
    <Sidebar {ctl} {s} />
  {:else if rail && Rail}
    <Rail {ctl} {s} />
  {:else if rail}
    <aside class="rail" aria-label="Comment threads" aria-busy="true"></aside>
  {/if}
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}

<!-- The drawer's box wider than a phone, with the artifact entry (not the
     gallery's): docked right (the default), left or at the bottom of the
     viewer, as the root's `data-drawer` says, sized by the custom
     properties on the root (`applyPrefs`, view/drawer-model.ts) and
     clamped here to the viewer with the limits view/drawer-model.ts clamps
     by (the greatest docked width, 720 px, is the stored width's own), so
     it fits from the first paint and on every resize. The rest of
     its rules, the floating window's box among them, come with its
     controls (DrawerChrome.svelte). At phone width it is the full-screen
     sheet (theme.css). The closed drawer's strip is 56 px wide, the
     viewer's height, on the drawer's side and bordered toward the page as
     the drawer is; its drawing's rules come with it (Rail.svelte). -->
<style>
  :global {
    /* The static frame's drawer, strip and the Clax app's view: the artifact
       page's alone, here rather than in theme.css, which the gallery carries too. */
    /* The drawer's box in the static frame, where the threads open docked
       (900 px and wider, ArtifactController's first `panel`), until the sidebar
       island draws its own. */
    .viewer > .drawer.held { display: none; }
    @media (min-width: 900px) { :root:not([data-drawer="float"], .app) .viewer > .drawer.held { display: block; } }
    /* The closed drawer's strip in the static frame (56 px, bordered toward the
       page, as SidebarIsland.svelte draws it) where it shows: 701 to 899 px
       wide, where the threads open closed, docked left or right; not where the
       bootstrap block, else the artifact's kept shape (view/drawer-shape.ts),
       says it has no thread (`data-rail="0"`, the inline clax-dsk script), nor
       in the Clax app's view. The sidebar island's own takes its place. */
    .viewer > .rail-held { display: none; flex: none; width: 56px; border: 0 solid var(--border); border-left-width: 1px; }
    [data-drawer="left"] .viewer > .rail-held { order: -1; border-width: 0 1px 0 0; }
    @media (min-width: 701px) and (max-width: 899px) { :root:not([data-drawer="bottom"], [data-drawer="float"], [data-rail="0"], .app) .viewer > .rail-held { display: block; } }
    /* The Clax app's view (`?app=1`, `html.app`, marked by the inline
       clax-theme script before the body is parsed): no chrome from the first
       paint, the stage filling the window (nor do the threads' drawer and
       strip show: each is the viewer's "Comment threads" aside, named so here
       rather than by the strip's own class, which ShellRail.svelte scopes). The rest of its rules come with the app's host module
       (app/app.css). */
    html.app .topbar, html.app .sidebar, html.app .phone-tabs, html.app .viewer aside[aria-label="Comment threads"] { display: none; }
    html.app .viewer { margin-bottom: 0; }
    /* The threads' skeleton over the drawer's box (view/drawer-shape.ts, drawn
       by the inline clax-dsk script): shown while the drawer has no list yet
       (the static frame's held drawer, the island's empty one) and while the
       list waits for its questions (`.page.cards-wait`, when the list and the
       page's pins are hidden too), so the cards and the pins show together. */
    .viewer > .dsk { display: none; position: absolute; top: 0; bottom: 0; right: 0; width: min(var(--drawer-w, 392px), 100% - 320px); min-width: 280px; overflow: hidden; pointer-events: none; z-index: 1; }
    [data-drawer="left"] .viewer > .dsk { right: auto; left: 0; }
    [data-drawer="bottom"] .viewer > .dsk { top: auto; left: 0; width: auto; min-width: 0; height: min(var(--drawer-h, 280px), 100% - 160px); min-height: 160px; }
    @media (min-width: 900px) { :root:not([data-drawer="float"]) :is(.viewer:has(> .drawer.held, > aside.drawer[aria-busy="true"]), .page.cards-wait .viewer) > .dsk { display: block; } }
    .dsk > i { position: absolute; left: 14px; right: 14px; border-radius: var(--radius); background: var(--hover); }
    .page.cards-wait .viewer .drawer aside.sidebar > *, .page.cards-wait .thread-pin { visibility: hidden; }
    @media (min-width: 701px) {
      .viewer .drawer { border: 0 solid var(--border); border-left-width: 1px; width: min(var(--drawer-w, 392px), 100% - 320px); min-width: 280px; }
      .viewer aside.rail { flex: none; width: 56px; border: 0 solid var(--border); border-left-width: 1px; }
      [data-drawer="left"] .drawer, [data-drawer="left"] aside.rail { order: -1; border-width: 0 1px 0 0; }
      [data-drawer="bottom"] .viewer { flex-direction: column; }
      [data-drawer="bottom"] .drawer { width: auto; height: min(var(--drawer-h, 280px), 100% - 160px); min-height: 160px; border-width: 1px 0 0; }
    }
  }
</style>
