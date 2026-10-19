<svelte:options css="injected" />

<script lang="ts">
  // The thread drawer around the sidebar (`children`), in the artifact view
  // only, never the extension's side panel. Its docked box is laid out by
  // the sidebar island's rules (SidebarIsland.svelte), its floating box by
  // these: the island draws a floating drawer only once this has loaded.
  // Its controls wider than a phone: a bar with the layout switch (dock
  // left, at the bottom, right, or float over the page) and Hide threads,
  // and the edges that resize the drawer. A docked drawer has one edge,
  // toward the page; the floating window moves by its bar and resizes by
  // its right and bottom edges and its corner. Each edge is a separator:
  // arrow keys move it, Shift further, Home and End to its least and
  // greatest size; a double click puts the size back to its default. While
  // a drag lasts the page takes no pointer (`.drawer-drag`), so the frame
  // cannot take the drag over. The prefs are this browser's
  // (drawer.svelte.ts), remembered as each change ends.
  import { type Snippet, tick } from "svelte";
  import { type DrawerLayout, type Grip, LAYOUTS, type Rect, type Size, keyMove, moved, reset, sizeOf } from "../view/drawer-model";
  import { drawer } from "./drawer.svelte";

  let { onClose, children }: { onClose(): void; children: Snippet } = $props();
  const p = $derived(drawer.prefs);
  const LABELS: Record<DrawerLayout, string> = { left: "Dock threads left", bottom: "Dock threads at the bottom", right: "Dock threads right", float: "Float threads over the page" };
  // The filled part of each layout's icon, in a 16 × 16 frame.
  const PANES: Record<DrawerLayout, string> = { left: "M3 3.5h3.5v9H3z", bottom: "M3 9h10v3.5H3z", right: "M9.5 3.5H13v9H9.5z", float: "M7.5 6H12v5H7.5z" };

  let bar: HTMLElement | undefined = $state();
  // The drawer's box in the viewer and the viewer's size, which the separators announce.
  let box: Rect = $state({ x: 0, y: 0, w: 0, h: 0 });
  let view: Size = $state({ w: 0, h: 0 });

  function measure(): { viewer: HTMLElement; box: Rect; view: Size } | null {
    const el = bar?.closest<HTMLElement>(".drawer");
    const viewer = el?.closest<HTMLElement>(".viewer");
    if (!el || !viewer) return null;
    const v = viewer.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    return { viewer, box: { x: r.left - v.left, y: r.top - v.top, w: r.width, h: r.height }, view: { w: v.width, h: v.height } };
  }
  // The stage's room beside the floating window, on the root for the
  // composer and the stage's foot to keep clear of it: on the side the
  // window leans to, the distance from that edge of the viewer to the far
  // side of the window.
  const clear = (side: "left" | "right", px: number) => document.documentElement.style.setProperty(`--float-clear-${side}`, px ? `${Math.round(px)}px` : "");
  const follow = () => {
    const m = measure();
    if (!m) return;
    box = m.box;
    view = m.view;
    const right = box.x + box.w / 2 >= view.w / 2;
    const floating = p.layout === "float";
    clear("right", floating && right ? view.w - box.x : 0);
    clear("left", floating && !right ? box.x + box.w : 0);
  };
  $effect(() => () => { clear("right", 0); clear("left", 0); });
  $effect(() => {
    const el = bar?.closest<HTMLElement>(".drawer");
    const viewer = el?.closest<HTMLElement>(".viewer");
    if (!el || !viewer || typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver(follow);
    ro.observe(el);
    ro.observe(viewer);
    return () => ro.disconnect();
  });
  // A move changes the place, not the size: measure once the change is drawn.
  $effect(() => { void drawer.prefs; void tick().then(follow); });

  function drag(e: PointerEvent, grip: Grip) {
    if (e.button !== 0) return;
    const m = measure();
    if (!m) return;
    e.preventDefault();
    const t = e.currentTarget as HTMLElement;
    try { t.setPointerCapture(e.pointerId); } catch { /* the pointer is gone */ }
    m.viewer.classList.add("drawer-drag");
    const from = p;
    const [x0, y0] = [e.clientX, e.clientY];
    const id = e.pointerId;
    const move = (ev: PointerEvent) => { if (ev.pointerId === id) drawer.set(moved(from, grip, m.box, ev.clientX - x0, ev.clientY - y0, m.view), false); };
    // The drag ends at the pointer's release, cancel or lost capture, heard
    // on the document (where lostpointercapture goes once the handle has
    // left the page), or at Escape, which puts the drawer back where it was;
    // and when the drawer unmounts. Release fires pointerup, then
    // lostpointercapture: it ends once.
    const up = (ev: PointerEvent) => { if (ev.pointerId === id) end(false); };
    const esc = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.preventDefault();
      ev.stopPropagation();
      end(true);
    };
    const on = (add: boolean) => {
      const f = add ? document.addEventListener.bind(document) : document.removeEventListener.bind(document);
      f("pointermove", move as EventListener, true);
      for (const type of ["pointerup", "pointercancel", "lostpointercapture"]) f(type, up as EventListener, true);
      f("keydown", esc as EventListener, true);
    };
    const end = (cancel: boolean) => {
      if (stopDrag !== end) return;
      stopDrag = null;
      on(false);
      try { if (t.hasPointerCapture(id)) t.releasePointerCapture(id); } catch { /* the pointer is gone */ }
      m.viewer.classList.remove("drawer-drag");
      drawer.set(cancel ? from : p);
    };
    stopDrag?.(false);
    stopDrag = end;
    on(true);
  }
  let stopDrag: ((cancel: boolean) => void) | null = null;
  $effect(() => () => stopDrag?.(false));

  // Docked left wider than a phone, the sidebar's island (this drawer's
  // parent, `display: contents`) goes before the stage in the page, so Tab
  // reaches the threads before the page as the eye does; otherwise, and
  // when the drawer unmounts, back after it. The stage, which holds the
  // frame, never moves. Focus inside the drawer is kept across the move.
  let root: HTMLElement | undefined = $state();
  const wide = typeof matchMedia === "function" ? matchMedia("(min-width: 701px)") : null;
  let isWide = $state(wide?.matches ?? true);
  $effect(() => {
    if (!wide) return;
    const f = () => { isWide = wide.matches; };
    wide.addEventListener("change", f);
    return () => wide.removeEventListener("change", f);
  });
  const place = (first: boolean) => {
    const island = root?.parentElement;
    const stage = island?.parentElement?.querySelector<HTMLElement>(":scope > .stage");
    if (!island?.classList.contains("island") || !stage) return;
    if (first ? island.nextElementSibling === stage : stage.nextElementSibling === island) return;
    const focused = island.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
    if (first) stage.before(island);
    else stage.after(island);
    focused?.focus();
  };
  $effect(() => place(p.layout === "left" && isWide));
  $effect(() => () => place(false));

  function key(e: KeyboardEvent, grip: Grip) {
    const d = keyMove(e.key, e.shiftKey, grip, p.layout);
    const m = d && measure();
    if (!d || !m) return;
    e.preventDefault();
    e.stopPropagation();
    drawer.set(moved(p, grip, m.box, d[0], d[1], m.view));
  }

  const choose = (layout: DrawerLayout) => drawer.set({ ...p, layout });
  // Hiding takes the focused button away with the drawer: focus goes to the
  // top bar's Threads toggle, which opens it again.
  const hide = () => {
    onClose();
    void tick().then(() => document.querySelector<HTMLElement>(".topbar button.threads")?.focus());
  };
  const barDown = (e: PointerEvent) => {
    if (p.layout === "float" && !(e.target as Element).closest("button")) drag(e, "move");
  };
  const edgeOf = (l: DrawerLayout) => (l === "left" ? "right" : l === "bottom" ? "top" : "left");
</script>

{#snippet edge(grip: Grip, side: string, label: string)}
  {@const v = sizeOf(grip, p, box, view)}
  <!-- A focusable separator is a widget (WAI-ARIA), which Svelte's check does not know. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="drawer-handle" data-grip={grip} data-edge={side} role="separator" tabindex="0" aria-label={label}
    aria-orientation={side === "top" || side === "bottom" ? "horizontal" : "vertical"} aria-valuenow={v?.now} aria-valuemin={v?.min} aria-valuemax={v?.max}
    title="Drag to resize; double-click to reset"
    onpointerdown={e => drag(e, grip)} onkeydown={e => key(e, grip)} ondblclick={() => drawer.set(reset(p, grip))}></div>
{/snippet}

<div class="drawer" bind:this={root}>
  <!-- The bar moves the floating window for a pointer; its move button does for the keys. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="drawer-bar" bind:this={bar} onpointerdown={barDown}>
    <span class="drawer-title">Threads</span>
    {#if p.layout === "float"}
      <button type="button" class="icon ghost drawer-move" aria-label="Move the threads window" title="Drag or use the arrow keys to move"
        onpointerdown={e => drag(e, "move")} onkeydown={e => key(e, "move")}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><g fill="currentColor"><circle cx="6" cy="4" r="1.2"/><circle cx="10" cy="4" r="1.2"/><circle cx="6" cy="8" r="1.2"/><circle cx="10" cy="8" r="1.2"/><circle cx="6" cy="12" r="1.2"/><circle cx="10" cy="12" r="1.2"/></g></svg>
      </button>
    {/if}
    <div class="drawer-layouts" role="group" aria-label="Threads layout">
      {#each LAYOUTS as l (l)}
        <button type="button" class="icon ghost" aria-pressed={p.layout === l} aria-label={LABELS[l]} title={LABELS[l]} onclick={() => choose(l)}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.75" y="2.25" width="12.5" height="11.5" rx="1.75" fill="none" stroke="currentColor" stroke-width="1.3"/><path d={PANES[l]} fill="currentColor"/></svg>
        </button>
      {/each}
    </div>
    <button type="button" class="icon ghost" aria-label="Hide threads" title="Hide threads" onclick={hide}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 4.5l7 7M11.5 4.5l-7 7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none"/></svg>
    </button>
  </div>
  {#if p.layout === "float"}
    {@render edge("fw", "right", "Resize threads window width")}
    {@render edge("fh", "bottom", "Resize threads window height")}
    <!-- Both edges at once, for a pointer; the keys use the edges. -->
    <div class="drawer-handle" data-grip="corner" aria-hidden="true" onpointerdown={e => drag(e, "corner")} ondblclick={() => drawer.set(reset(p, "corner"))}></div>
  {:else}
    {@render edge("dock", edgeOf(p.layout), p.layout === "bottom" ? "Resize threads height" : "Resize threads width")}
  {/if}
  {@render children()}
</div>

<style>
  :global {
    /* At phone width the drawer is no box at all, and the sidebar is the
       full-screen sheet (theme.css). */
    div.drawer { display: contents; }
    .drawer-bar, .drawer-handle { display: none; }
    @media (min-width: 701px) {
      .viewer div.drawer { position: relative; display: flex; flex-direction: column; flex: none; }
      .drawer > .sidebar { flex: 1; width: auto; min-height: 0; border: 0; }
      /* Over the page and above the composer and the stage's banners, which
         keep to the stage's room beside it (`--float-clear-*`); by default
         48 px in from the right, clear of the pins (pins-model.ts keeps them
         PIN_RIGHT_ROOM, 38 px, off the stage's right edge); its place and
         size clamped to the viewer. */
      [data-drawer="float"] .viewer div.drawer { --fw: clamp(280px, var(--drawer-fw, 392px), 100%); --fh: clamp(200px, var(--drawer-fh, 100% - 32px), 100%);
        position: absolute; z-index: 6; width: var(--fw); height: var(--fh); overflow: hidden;
        left: clamp(0px, var(--drawer-x, 100% - var(--fw) - 48px), 100% - var(--fw)); top: clamp(0px, var(--drawer-y, 16px), 100% - var(--fh));
        border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
      [data-drawer="float"] .composer { right: clamp(16px, var(--float-clear-right, 0px) + 16px, 100% - min(360px, 100% - 32px) - 16px); }
      [data-drawer="float"] .stage-foot { left: calc(var(--float-clear-left, 0px) + 16px); right: calc(var(--float-clear-right, 0px) + 16px); }
      .drawer-bar { display: flex; align-items: center; gap: 2px; flex: none; height: 40px; padding: 0 6px 0 14px; border-bottom: 1px solid var(--border); background: var(--bg); }
      [data-drawer="float"] .drawer .drawer-bar { cursor: move; background: var(--card); touch-action: none; }
      .drawer-title { flex: 1; min-width: 0; font: 600 13px/1.2 var(--font); color: var(--muted); }
      .drawer-layouts { display: flex; gap: 2px; margin-right: 4px; padding-right: 6px; border-right: 1px solid var(--border); }
      .drawer-bar button.icon { width: 28px; min-height: 28px; color: var(--muted); }
      .drawer-bar button.icon:not(:disabled):hover { color: var(--fg); }
      .drawer-bar button[aria-pressed="true"], .drawer-bar button[aria-pressed="true"]:not(:disabled):hover { background: var(--hover); color: var(--fg); }
      .drawer-bar .drawer-move { cursor: move; touch-action: none; }
      .drawer-handle { display: block; position: absolute; z-index: 2; touch-action: none; }
      .drawer-handle:focus-visible { outline: none; }
      .drawer-handle[data-edge="left"], .drawer-handle[data-edge="right"] { top: 0; bottom: 0; width: 8px; cursor: col-resize; }
      .drawer-handle[data-edge="top"], .drawer-handle[data-edge="bottom"] { left: 0; right: 0; height: 8px; cursor: row-resize; }
      .drawer-handle[data-edge="left"] { left: -5px; }
      .drawer-handle[data-edge="right"] { right: -5px; }
      .drawer-handle[data-edge="top"] { top: -5px; }
      /* The floating window clips what overflows it: its edges sit inside. */
      [data-drawer="float"] .drawer .drawer-handle[data-edge="right"] { right: 0; width: 6px; }
      [data-drawer="float"] .drawer .drawer-handle[data-edge="bottom"] { bottom: 0; height: 6px; }
      .drawer-handle[data-grip="corner"] { right: 0; bottom: 0; width: 16px; height: 16px; cursor: nwse-resize; z-index: 3; }
      .drawer-handle[data-grip="corner"]::before { content: ""; position: absolute; right: 3px; bottom: 3px; width: 8px; height: 8px; border-right: 2px solid var(--border-strong); border-bottom: 2px solid var(--border-strong); border-bottom-right-radius: 3px; }
      /* A line along the edge shows where it is, on hover, while dragged and with focus. */
      .drawer-handle[role="separator"]::after { content: ""; position: absolute; background: transparent; transition: background-color var(--t); }
      .drawer-handle[data-edge="left"]::after, .drawer-handle[data-edge="right"]::after { top: 0; bottom: 0; left: calc(50% - 1px); width: 2px; }
      .drawer-handle[data-edge="top"]::after, .drawer-handle[data-edge="bottom"]::after { left: 0; right: 0; top: calc(50% - 1px); height: 2px; }
      [data-drawer="float"] .drawer .drawer-handle[data-edge="right"]::after { left: auto; right: 0; }
      [data-drawer="float"] .drawer .drawer-handle[data-edge="bottom"]::after { top: auto; bottom: 0; }
      .drawer-handle[role="separator"]:hover::after, .viewer.drawer-drag .drawer-handle[role="separator"]:active::after { background: var(--border-strong); }
      .drawer-handle[role="separator"]:focus-visible::after { background: var(--focus); }
      .viewer.drawer-drag { user-select: none; -webkit-user-select: none; }
      .viewer.drawer-drag .stage, .viewer.drawer-drag .stage * { pointer-events: none !important; }
    }
  }
</style>
