<svelte:options css="injected" />

<script lang="ts">
  // A browser session's recording, played as a flipbook (spec
  // 2026-11-19-claxbot-browser §6.6, §12): the clip is a frame manifest of
  // the run's content (`{width, height, frames: [{item, at_ms}]}`), each
  // frame a JPEG item of the run, shown at its time. It shows its first
  // frame until Play; it stops at its last; Play again starts it over.
  // Pause stops it where it is. Nothing plays by itself, so a reader who
  // asked for less motion sees a still frame until they choose to play it.
  import { onDestroy } from "svelte";

  let { run, clip, label }: {
    /** The run whose content holds the clip and its frames. */
    run: string;
    /** The clip's content ID: its frame manifest. */
    clip: string;
    /** What the flipbook is called to a screen reader ("Recording 1"). */
    label: string;
  } = $props();

  type Frame = { item: string; at_ms: number };
  const item = (id: string) => `/api/bot/runs/${encodeURIComponent(run)}/content/${encodeURIComponent(id)}`;
  let frames = $state<Frame[]>([]);
  let size = $state<{ width: number; height: number } | null>(null);
  let at = $state(0);
  let playing = $state(false);
  let failed = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const url = item(clip);
    let gone = false;
    void fetch(url, { credentials: "same-origin" }).then(async r => {
      if (!r.ok) throw new Error(String(r.status));
      const m = await r.json() as { width?: number; height?: number; frames?: Frame[] };
      if (gone) return;
      frames = Array.isArray(m.frames) ? m.frames.filter(f => typeof f?.item === "string") : [];
      size = m.width && m.height ? { width: m.width, height: m.height } : null;
      at = 0;
    }).catch(() => { if (!gone) failed = true; });
    return () => { gone = true; stop(); };
  });

  function stop() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    playing = false;
  }

  /** Shows the next frame at its time after the one shown, until the last. */
  function next() {
    if (at >= frames.length - 1) { stop(); return; }
    const wait = Math.max(0, (frames[at + 1].at_ms ?? 0) - (frames[at].at_ms ?? 0));
    timer = setTimeout(() => { timer = null; at += 1; next(); }, wait);
  }

  function toggle() {
    if (playing) { stop(); return; }
    if (at >= frames.length - 1) at = 0;
    playing = true;
    next();
  }

  onDestroy(stop);
</script>

{#if failed}
  <span class="flip-gone">The recording is not available.</span>
{:else if frames.length}
  <figure class="flip" role="group" aria-label={label}>
    <img src={item(frames[at].item)} alt={`Frame ${at + 1} of ${frames.length}`} width={size?.width} height={size?.height} />
    <figcaption>
      <button type="button" class="ghost" onclick={toggle}>{playing ? "Pause" : "Play"}</button>
      <span class="at">{at + 1} / {frames.length}</span>
    </figcaption>
  </figure>
{/if}

<style>
  :global {
    .flip { margin: 4px 0 0; display: inline-grid; gap: 4px; justify-items: start; }
    .flip img { display: block; width: auto; height: 120px; max-width: 220px; object-fit: contain; border: 1px solid var(--border); border-radius: var(--radius-xs); background: var(--hover); }
    .flip figcaption { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); }
    .flip .at { font-variant-numeric: tabular-nums; }
    .flip-gone { font-size: 12px; color: var(--muted); }
  }
</style>
