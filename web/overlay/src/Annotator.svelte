<script lang="ts">
  // The capture with the reporter's marks (spec 2026-10-29-feedback-loop
  // §4.3): Box, Arrow, Redact (an opaque block) and Text, drawn with the
  // pointer over the image or from the keyboard (`keyStep`: the arrow keys
  // move a crosshair, Enter sets each corner, Escape drops one), "Redact
  // the pick" in one step, and Undo and Clear. The marks are kept in the
  // image's own pixels and drawn here as SVG over it, as `annotate.ts`
  // paints them into the PNG that is sent; a redaction shows opaque here
  // as it is sent.
  //
  // The image and its drawing share one box (`.canvas`) that scrolls inside
  // the stage as a whole, so a point on the drawing is the same point of the
  // image however tall the image is and however far it is scrolled.
  import { ACCENT, type KeyDraw, MAX_TEXT, ON_ACCENT, REDACT_FILL, STROKE_PX, TEXT_PX, type Pt, type Shape, type Tool, barbs, keyStep, plateOf, spanOf, textWidth } from "./annotate";

  type Props = {
    src: string;
    alt: string;
    shapes: Shape[];
    scale: number;
    /** The pick in the image's pixels, for "Redact the pick"; none hides it. */
    pick?: { x: number; y: number; w: number; h: number } | null;
    onChange(shapes: Shape[]): void;
  };
  let { src, alt, shapes, scale, pick = null, onChange }: Props = $props();

  let tool = $state<Tool>("box");
  let size = $state({ w: 0, h: 0 });
  let drawing = $state<{ kind: "box" | "arrow" | "redact"; from: Pt; to: Pt } | null>(null);
  /** Where a text mark is being typed, and what has been typed. */
  let typing = $state<Pt | null>(null);
  let typed = $state("");
  let keys = $state<KeyDraw | null>(null);
  let said = $state("");
  let svg = $state<SVGSVGElement | null>(null);
  let stage = $state<HTMLElement | null>(null);

  const TOOLS: { tool: Tool; label: string }[] = [
    { tool: "box", label: "Box" },
    { tool: "arrow", label: "Arrow" },
    { tool: "redact", label: "Redact" },
    { tool: "text", label: "Text" },
  ];

  const s = $derived(Number.isFinite(scale) && scale > 0 ? scale : 1);
  /** The image's size, or before it has loaded what the marks reach. */
  const box = $derived(size.w > 0 && size.h > 0 ? size : { w: Math.max(1, pick ? pick.x + pick.w : 0, ...shapes.map(sh => reach(sh))), h: Math.max(1, pick ? pick.y + pick.h : 0, ...shapes.map(sh => reach(sh, "y"))) });
  function reach(sh: Shape, axis: "x" | "y" = "x"): number {
    return sh.kind === "text" ? sh.at[axis] : Math.max(sh.from[axis], sh.to[axis]);
  }

  /** Where the pointer is, in the image's pixels. */
  function pointOf(e: PointerEvent): Pt {
    const r = svg?.getBoundingClientRect();
    if (!r || r.width === 0 || r.height === 0) return { x: 0, y: 0 };
    const clamp = (v: number, max: number) => Math.min(Math.max(v, 0), max);
    return { x: Math.round(clamp(((e.clientX - r.left) / r.width) * box.w, box.w)), y: Math.round(clamp(((e.clientY - r.top) / r.height) * box.h, box.h)) };
  }

  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    e.preventDefault();
    keys = null;
    const p = pointOf(e);
    if (tool === "text") { typing = p; typed = ""; return; }
    drawing = { kind: tool, from: p, to: p };
    (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
  }
  function move(e: PointerEvent): void {
    if (drawing) drawing = { ...drawing, to: pointOf(e) };
  }
  function up(e: PointerEvent): void {
    const d = drawing;
    drawing = null;
    if (!d) return;
    const to = pointOf(e);
    const r = spanOf(d.from, to);
    // A click draws nothing.
    if (r.w < 4 && r.h < 4) return;
    onChange([...shapes, { kind: d.kind, from: d.from, to }]);
  }

  function commitText(): void {
    const at = typing;
    if (!at) return;
    typing = null;
    const text = typed.trim().slice(0, MAX_TEXT);
    typed = "";
    if (text) { onChange([...shapes, { kind: "text", at, text }]); said = "Text added."; }
    stage?.focus();
  }

  function onKey(e: KeyboardEvent): void {
    if (e.target !== stage || e.altKey || e.metaKey || e.ctrlKey) return;
    const st = keys ?? { cursor: { x: Math.round(box.w / 2), y: Math.round(box.h / 2) }, corner: null };
    const r = keyStep(st, e.key, e.shiftKey, tool, box);
    if (!r.used) return;
    e.preventDefault();
    e.stopPropagation();
    keys = r.state;
    if (r.say) said = r.say;
    if (r.add) onChange([...shapes, r.add]);
    if (r.askText) { typing = r.askText; typed = ""; }
  }

  function redactPick(): void {
    if (!pick) return;
    onChange([...shapes, { kind: "redact", from: { x: pick.x, y: pick.y }, to: { x: pick.x + pick.w, y: pick.y + pick.h } }]);
    said = "The pick is redacted.";
  }

  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
  const toolName = $derived(TOOLS.find(t => t.tool === tool)?.label ?? "");
</script>

<figure class="annotator">
  <div class="tools" role="toolbar" aria-label="Mark the screenshot">
    {#each TOOLS as t (t.tool)}
      <button type="button" aria-pressed={tool === t.tool} onclick={() => { tool = t.tool; keys = keys && { ...keys, corner: null }; }}>{t.label}</button>
    {/each}
    {#if pick}<button type="button" onclick={redactPick}>Redact the pick</button>{/if}
    <button type="button" disabled={shapes.length === 0} onclick={() => { onChange(shapes.slice(0, -1)); said = "Last mark removed."; }}>Undo</button>
    <button type="button" disabled={shapes.length === 0} onclick={() => { onChange([]); said = "All marks removed."; }}>Clear</button>
  </div>
  <!-- The stage is a drawing widget: it takes the keys itself (keyStep). -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="stage" bind:this={stage} tabindex="0" role="application" aria-roledescription="drawing area"
    aria-label={`Screenshot, ${shapes.length === 1 ? "1 mark" : `${shapes.length} marks`}. ${toolName} tool: arrow keys move the crosshair, Enter sets each corner, Escape cancels.`}
    onkeydown={onKey}>
    <div class="canvas">
      <img {src} {alt} draggable="false" onload={e => { const i = e.currentTarget as HTMLImageElement; size = { w: i.naturalWidth, h: i.naturalHeight }; }} />
      <svg bind:this={svg} viewBox={`0 0 ${box.w} ${box.h}`} preserveAspectRatio="none" aria-hidden="true"
        class:text={tool === "text"} onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={() => { drawing = null; }}>
        {#each drawing ? [...shapes, drawing] : shapes as sh, i (i)}
          {#if sh.kind === "redact"}
            {@const r = spanOf(sh.from, sh.to)}
            <rect data-kind="redact" x={r.x} y={r.y} width={r.w} height={r.h} fill={REDACT_FILL} />
          {:else if sh.kind === "box"}
            {@const r = spanOf(sh.from, sh.to)}
            <rect data-kind="box" x={r.x} y={r.y} width={r.w} height={r.h} fill="none" stroke={ACCENT} stroke-width={STROKE_PX * s} stroke-linejoin="round" />
          {:else if sh.kind === "arrow"}
            {@const [a, b] = barbs(sh.from, sh.to, STROKE_PX * s * 4)}
            <g data-kind="arrow">
              <line x1={sh.from.x} y1={sh.from.y} x2={sh.to.x} y2={sh.to.y} stroke={ACCENT} stroke-width={STROKE_PX * s} stroke-linecap="round" />
              <polygon points={`${sh.to.x},${sh.to.y} ${a.x},${a.y} ${b.x},${b.y}`} fill={ACCENT} />
            </g>
          {:else if sh.kind === "text"}
            {@const size = TEXT_PX * s}
            {@const p = plateOf(sh.at, textWidth(sh.text, size), size)}
            <g data-kind="text">
              <rect x={p.x} y={p.y} width={p.w} height={p.h} fill={ACCENT} />
              <text x={p.x + p.pad} y={p.y + p.pad + size * 0.15} dominant-baseline="hanging" font-size={size} font-weight="600" fill={ON_ACCENT}>{sh.text}</text>
            </g>
          {/if}
        {/each}
        {#if keys}
          {@const k = STROKE_PX * s * 3}
          {#if keys.corner}
            {@const r = spanOf(keys.corner, keys.cursor)}
            <rect class="pending" x={r.x} y={r.y} width={r.w} height={r.h} fill="none" stroke={ACCENT} stroke-width={STROKE_PX * s} stroke-dasharray={`${k} ${k}`} />
          {/if}
          <g class="crosshair" data-x={keys.cursor.x} data-y={keys.cursor.y}>
            <line x1={keys.cursor.x - k} y1={keys.cursor.y} x2={keys.cursor.x + k} y2={keys.cursor.y} stroke={ACCENT} stroke-width={STROKE_PX * s * 0.6} />
            <line x1={keys.cursor.x} y1={keys.cursor.y - k} x2={keys.cursor.x} y2={keys.cursor.y + k} stroke={ACCENT} stroke-width={STROKE_PX * s * 0.6} />
          </g>
        {/if}
      </svg>
      {#if typing}
        <input class="typing" type="text" aria-label="Text to add" maxlength={MAX_TEXT} placeholder="Type, then Enter" style:left={pct(typing.x, box.w)} style:top={pct(typing.y, box.h)}
          bind:value={typed} {@attach el => el.focus()} onblur={commitText}
          onkeydown={e => {
            if (e.key === "Enter") { e.preventDefault(); commitText(); }
            else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); typing = null; stage?.focus(); }
          }} />
      {/if}
    </div>
  </div>
  <p class="said" aria-live="polite">{said}</p>
</figure>

<style>
  .annotator { margin: 0 0 8px; }
  .tools { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 4px; }
  .tools button { min-height: 26px; padding: 3px 8px; font-size: 12px; }
  .tools button[aria-pressed="true"] { background: var(--hover); border-color: var(--border-strong); }
  .stage { border: 1px solid var(--border-strong); border-radius: var(--radius-xs); background: var(--bg); line-height: 0; max-height: 240px; overflow: auto; }
  .stage:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
  /* The image's own box: the drawing covers exactly the image, wherever the stage is scrolled. */
  .canvas { position: relative; }
  img { display: block; width: 100%; height: auto; min-height: 24px; user-select: none; }
  svg { position: absolute; inset: 0; width: 100%; height: 100%; cursor: crosshair; touch-action: none; }
  svg.text { cursor: text; }
  .typing { position: absolute; width: 160px; font: 13px/1.3 var(--font); padding: 2px 6px; color: var(--fg); background: var(--card); border: 1px solid var(--focus); border-radius: var(--radius-xs); }
  .said { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
</style>
