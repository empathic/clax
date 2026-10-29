<script lang="ts">
  // The capture with the reporter's marks (spec 2026-10-29-feedback-loop
  // §4.3): Box, Arrow, Redact (an opaque block) and Text, drawn with the
  // pointer over the image, and Undo and Clear. The marks are kept in the
  // image's own pixels and drawn here as SVG over it, as `annotate.ts`
  // paints them into the PNG that is sent; a redaction shows opaque here
  // as it is sent.
  import { ACCENT, MAX_TEXT, ON_ACCENT, REDACT_FILL, STROKE_PX, TEXT_PX, type Pt, type Shape, barbs, plateOf, spanOf } from "./annotate";

  type Tool = "box" | "arrow" | "redact" | "text";
  type Props = { src: string; alt: string; shapes: Shape[]; scale: number; onChange(shapes: Shape[]): void };
  let { src, alt, shapes, scale, onChange }: Props = $props();

  let tool = $state<Tool>("box");
  let size = $state({ w: 0, h: 0 });
  let drawing = $state<{ kind: "box" | "arrow" | "redact"; from: Pt; to: Pt } | null>(null);
  let typing = $state<{ at: Pt; text: string } | null>(null);
  let svg = $state<SVGSVGElement | null>(null);

  const TOOLS: { tool: Tool; label: string }[] = [
    { tool: "box", label: "Box" },
    { tool: "arrow", label: "Arrow" },
    { tool: "redact", label: "Redact" },
    { tool: "text", label: "Text" },
  ];

  const s = $derived(Number.isFinite(scale) && scale > 0 ? scale : 1);
  /** The marks' extent where the image has not loaded yet: what they reach. */
  const box = $derived(size.w > 0 && size.h > 0 ? size : { w: Math.max(1, ...shapes.map(sh => reach(sh))), h: Math.max(1, ...shapes.map(sh => reach(sh, "y"))) });
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
    const p = pointOf(e);
    if (tool === "text") { typing = { at: p, text: "" }; return; }
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
    const t = typing;
    typing = null;
    const text = t?.text.trim().slice(0, MAX_TEXT);
    if (t && text) onChange([...shapes, { kind: "text", at: t.at, text }]);
  }

  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
</script>

<figure class="annotator">
  <div class="tools" role="toolbar" aria-label="Mark the screenshot">
    {#each TOOLS as t (t.tool)}
      <button type="button" aria-pressed={tool === t.tool} onclick={() => { tool = t.tool; }}>{t.label}</button>
    {/each}
    <button type="button" disabled={shapes.length === 0} onclick={() => onChange(shapes.slice(0, -1))}>Undo</button>
    <button type="button" disabled={shapes.length === 0} onclick={() => onChange([])}>Clear</button>
  </div>
  <div class="stage">
    <img {src} {alt} draggable="false" onload={e => { const i = e.currentTarget as HTMLImageElement; size = { w: i.naturalWidth, h: i.naturalHeight }; }} />
    <svg bind:this={svg} viewBox={`0 0 ${box.w} ${box.h}`} preserveAspectRatio="none" role="img" aria-label={shapes.length === 1 ? "1 mark" : `${shapes.length} marks`}
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
          {@const p = plateOf(sh.at, sh.text.length * size * 0.56, size)}
          <g data-kind="text">
            <rect x={p.x} y={p.y} width={p.w} height={p.h} fill={ACCENT} />
            <text x={p.x + p.pad} y={p.y + p.pad + size * 0.15} dominant-baseline="hanging" font-size={size} font-weight="600" fill={ON_ACCENT}>{sh.text}</text>
          </g>
        {/if}
      {/each}
    </svg>
    {#if typing}
      <input class="typing" type="text" aria-label="Text to add" maxlength={MAX_TEXT} placeholder="Type, then Enter" style:left={pct(typing.at.x, box.w)} style:top={pct(typing.at.y, box.h)}
        bind:value={typing.text} {@attach el => el.focus()} onblur={commitText}
        onkeydown={e => {
          if (e.key === "Enter") { e.preventDefault(); commitText(); }
          else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); typing = null; }
        }} />
    {/if}
  </div>
</figure>

<style>
  .annotator { margin: 0 0 8px; }
  .tools { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 4px; }
  .tools button { min-height: 26px; padding: 3px 8px; font-size: 12px; }
  .tools button[aria-pressed="true"] { background: var(--hover); border-color: var(--border-strong); }
  .stage { position: relative; border: 1px solid var(--border-strong); border-radius: var(--radius-xs); background: var(--bg); line-height: 0; max-height: 240px; overflow: auto; }
  img { display: block; width: 100%; height: auto; min-height: 24px; user-select: none; }
  svg { position: absolute; inset: 0; width: 100%; height: 100%; cursor: crosshair; touch-action: none; }
  svg.text { cursor: text; }
  .typing { position: absolute; width: 160px; font: 13px/1.3 var(--font); padding: 2px 6px; color: var(--fg); background: var(--card); border: 1px solid var(--focus); border-radius: var(--radius-xs); }
</style>
