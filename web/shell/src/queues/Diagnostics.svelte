<svelte:options css="injected" />

<script lang="ts">
  // A capture's diagnostics (spec 2026-10-29-feedback-loop-design §5.2):
  // collapsed under how many lines it holds, fetched (`load`) the first
  // time it opens (and again by Try again after a failed fetch), and
  // filtered by level (every level, or one). Each line is the reporter's
  // browser's, shown as text: its time, level, kind and text.
  import type { DiagLine } from "../../../sdk/src/types";
  import { LEVELS, refusal } from "./model";

  const p: { count: number; label?: string; load: () => Promise<DiagLine[]> } = $props();
  let lines = $state<DiagLine[] | null>(null);
  let err = $state<string | null>(null);
  let level = $state<DiagLine["level"] | "all">("all");
  const name = `diag-${Math.random().toString(36).slice(2)}`;
  const shown = $derived(lines?.filter(l => level === "all" || l.level === level) ?? []);
  const of = (lv: DiagLine["level"]) => lines?.filter(l => l.level === lv).length ?? 0;
  const KIND = { console: "console", error: "uncaught error", rejection: "unhandled rejection", request: "failed request" } as const;
  const clock = (at: string) => /T(\d\d:\d\d:\d\d(?:\.\d+)?)/.exec(at)?.[1] ?? at;

  let loading = false;
  function fetchLines() {
    if (loading) return;
    loading = true;
    err = null;
    p.load().then(l => { lines = l; }, x => { err = `Could not load the diagnostics: ${refusal(x)}`; }).finally(() => { loading = false; });
  }
  function toggle(e: Event) {
    if ((e.currentTarget as HTMLDetailsElement).open && !lines && !err) fetchLines();
  }
</script>

<details class="diag" ontoggle={toggle}>
  <summary>{p.label ?? "Diagnostics"} <span class="n">{p.count} {p.count === 1 ? "line" : "lines"}</span></summary>
  {#if err}<p class="err" role="alert">{err} <button type="button" class="ghost" onclick={fetchLines}>Try again</button></p>
  {:else if !lines}<p class="muted" role="status">Loading…</p>
  {:else}
    <fieldset class="levels">
      <legend>Level</legend>
      <label><input type="radio" {name} value="all" bind:group={level} /> All <span class="n">{lines.length}</span></label>
      {#each LEVELS as lv (lv)}
        <label><input type="radio" {name} value={lv} bind:group={level} /> {lv} <span class="n">{of(lv)}</span></label>
      {/each}
    </fieldset>
    {#if shown.length}
      <ol class="lines">
        {#each shown as l, i (i)}
          <li class={["line", l.level]} data-level={l.level}>
            <span class="at" title={l.at}>{clock(l.at)}</span>
            <span class="lv">{l.level}</span>
            <span class="kind">{KIND[l.kind] ?? l.kind}</span>
            <span class="text">{l.text}</span>
          </li>
        {/each}
      </ol>
    {:else}
      <p class="muted">No {level === "all" ? "" : `${level} `}lines.</p>
    {/if}
  {/if}
</details>

<style>
  .diag { min-width: 0; }
  .diag summary { cursor: pointer; font-size: 13.5px; font-weight: 600; }
  .n { color: var(--muted); font-weight: 400; font-variant-numeric: tabular-nums; font-size: 12px; }
  .muted { color: var(--muted); font-size: 13px; }
  .err { color: var(--danger); font-size: 13px; }
  .levels { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 8px 0; padding: 0; border: 0; font-size: 12.5px; }
  .levels legend { float: left; margin-right: 8px; color: var(--muted); }
  .levels label { display: inline-flex; align-items: center; gap: 4px; }
  .lines { list-style: none; margin: 0; padding: 0; max-height: 420px; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); font: 12px/1.45 var(--mono); }
  .line { display: grid; grid-template-columns: max-content max-content max-content minmax(0, 1fr); gap: 0 10px; padding: 3px 8px; border-bottom: 1px solid var(--border); }
  .line:last-child { border-bottom: 0; }
  .line .at, .line .kind { color: var(--muted); }
  .line .lv { text-transform: uppercase; font-size: 11px; }
  .line.error .lv { color: var(--danger); }
  .line.warn .lv { color: var(--tag-orange-fg); }
  .text { white-space: pre-wrap; overflow-wrap: anywhere; }
  @media (max-width: 600px) {
    .line { grid-template-columns: max-content max-content minmax(0, 1fr); }
    .line .kind { display: none; }
  }
  @media (pointer: coarse) { .levels label { min-height: 44px; } }
</style>
