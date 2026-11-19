<svelte:options css="injected" />
<script lang="ts">
  // The Analytics panel (the more menu's Analytics…, spec
  // 2026-11-19-site-analytics-design): a hosted clax's site visits over a
  // range of UTC days, for whoever administers it (the artifact carries
  // `analytics` only for them, on a hosted Clax, never on a draft; the top
  // bar mounts the panel only then). The range picker (analytics-model.ts
  // `RANGES`), the totals, a visits-per-day chart drawn here as inline SVG
  // with a mark at each version published and a table of the same numbers
  // for screen readers, a table per dimension, and the setting: a line
  // saying whether Clax counts visits and one button that changes it at
  // once (`analytics-api.ts`, no approval). Its own lazy chunk; on mount it
  // gives the more menu its opener (`setAnalyticsOpener`).
  import { onMount } from "svelte";
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import { failureText } from "../failure";
  import { readAnalytics, setAnalytics } from "./analytics-api";
  import {
    type AnalyticsAnswer, type AnalyticsSetting, type RangeKey,
    CHANGE_FAILED, CHART, DAYS_NOTE, DEFAULT_RANGE, LCP, LOAD, LOAD_FAILED, LOADING, RANGE_LABEL, RANGES, SETTING_HEADING,
    TABLE_VIEWS, TABLE_VISITORS, TABLES, TITLE, VIEWS, VISITORS, VISITORS_NOTE,
    chart, chartSummary, chartSvg, dayLabel, emptyText, formatCount, formatMs, framedText, rangeFrom, rowLabel, settingWords, utcDay, withYear,
  } from "./analytics-model";
  import { setAnalyticsOpener } from "./more-menu.svelte";
  import { inertOutside, trapTab } from "./modal";

  // `today`: the UTC day the range is counted back from (tests pass their own).
  let { ctl, today = () => utcDay(Date.now()) }: { ctl: ArtifactController; today?: () => string } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const title = $derived(view.current.data?.artifact.title ?? "");

  let open = $state(false);
  let range = $state<RangeKey>(DEFAULT_RANGE);
  let answer = $state<AnalyticsAnswer | null>(null);
  let setting = $state<AnalyticsSetting | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let panel: HTMLElement | undefined = $state();
  const back = { el: null as HTMLElement | null };
  // The latest read wins: an older range's answer arriving late is dropped.
  let asked = 0;

  async function load(): Promise<void> {
    const mine = ++asked;
    try {
      const a = await readAnalytics(ctl.id, rangeFrom(range, today()));
      if (mine !== asked) return;
      answer = a;
      setting = a.analytics;
      error = null;
    } catch (e) {
      if (mine === asked) error = failureText(LOAD_FAILED, e);
    }
  }
  function choose(r: RangeKey) {
    if (r === range) return;
    range = r;
    void load();
  }
  async function toggle() {
    if (busy || !setting) return;
    busy = true;
    const to: AnalyticsSetting = setting === "on" ? "off" : "on";
    try {
      const r = await setAnalytics(ctl.id, to);
      setting = r.artifact?.analytics ?? to;
      error = null;
    } catch (e) {
      error = failureText(CHANGE_FAILED, e);
    } finally {
      busy = false;
    }
  }
  function show() {
    back.el = document.activeElement as HTMLElement | null;
    open = true;
    void load();
  }
  function close() {
    open = false;
    if (back.el?.isConnected) back.el.focus?.();
  }
  onMount(() => {
    setAnalyticsOpener(show);
    return () => setAnalyticsOpener(null);
  });

  const a = $derived(answer);
  const empty = $derived(a ? emptyText(a, range) : null);
  const c = $derived(a ? chart(a) : null);
  const year = $derived(a ? withYear(a) : false);
  const words = $derived(setting ? settingWords(setting) : null);

  // The chart's markup (analytics-model.ts `chartSvg`), set as the answer changes.
  const draw = (el: HTMLElement) => { el.innerHTML = c ? chartSvg(c) : ""; };
  const modal = (el: HTMLElement) => inertOutside(el);
  const focus = (el: HTMLElement) => { el.focus(); };
  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); return; }
    if (panel) trapTab(e, panel);
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="an-backdrop" {@attach modal} onclick={e => { if (e.target === e.currentTarget) close(); }}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="an-panel" role="dialog" aria-modal="true" aria-labelledby="an-title" tabindex="-1" bind:this={panel} onkeydown={keydown} data-testid="analytics-panel">
      <h2 id="an-title">{TITLE(title)}</h2>
      <div class="an-range" role="group" aria-label={RANGE_LABEL}>
        {#each RANGES as r (r.key)}
          <button type="button" aria-pressed={range === r.key} onclick={() => choose(r.key)}>{r.label}</button>
        {/each}
      </div>
      {#if error}<p class="err" role="alert">{error}</p>{/if}
      {#if !a && !error}<p class="muted">{LOADING}</p>{/if}
      {#if a && empty !== null}
        {#if empty}<p class="an-empty" data-testid="analytics-empty">{empty}</p>{/if}
      {:else if a && c}
        <dl class="an-totals" data-testid="analytics-totals">
          <div><dt>{VISITORS}</dt><dd data-total="visitors">{formatCount(a.totals.visitors)}</dd></div>
          <div><dt>{VIEWS}</dt><dd data-total="views">{formatCount(a.totals.views)}</dd></div>
          <div><dt>{LOAD}</dt><dd data-total="load">{formatMs(a.totals.load_ms_median)}</dd></div>
          {#if a.totals.lcp_ms_median !== null}<div><dt>{LCP}</dt><dd data-total="lcp">{formatMs(a.totals.lcp_ms_median)}</dd></div>{/if}
        </dl>
        <p class="small muted">{VISITORS_NOTE} {framedText(a.totals) ?? ""} {DAYS_NOTE}</p>
        <figure class="an-chart" data-testid="analytics-chart">
          <figcaption>
            <span>{CHART}</span>
            <span class="an-key"><i class="sw views"></i>{VIEWS}</span>
            <span class="an-key"><i class="sw visitors"></i>{VISITORS}</span>
          </figcaption>
          <div class="an-plot">
            <span class="an-max" aria-hidden="true">{formatCount(c.max)}</span>
            <div class="an-draw" {@attach draw}></div>
          </div>
          <div class="an-axis" aria-hidden="true"><span>{dayLabel(a.from, year)}</span><span>{dayLabel(a.to, year)}</span></div>
          <p class="an-sr" data-testid="analytics-chart-summary">{chartSummary(a)}</p>
          <table class="an-sr" data-testid="analytics-chart-table">
            <caption>{CHART}</caption>
            <thead><tr><th scope="col">Day</th><th scope="col">{VISITORS}</th><th scope="col">{VIEWS}</th></tr></thead>
            <tbody>{#each a.days as d (d.day)}<tr><th scope="row">{dayLabel(d.day, year)}</th><td>{formatCount(d.visitors)}</td><td>{formatCount(d.views)}</td></tr>{/each}</tbody>
          </table>
        </figure>
        {#each TABLES as t (t.kind)}
          {#if a[t.kind].length}
            <section class="an-table" data-table={t.kind}>
              <h3 id={`an-${t.kind}`}>{t.heading}</h3>
              <table aria-labelledby={`an-${t.kind}`}>
                <thead><tr><th scope="col">{t.column}</th><th scope="col" class="num">{TABLE_VISITORS}</th><th scope="col" class="num">{TABLE_VIEWS}</th></tr></thead>
                <tbody>
                  {#each a[t.kind] as r (r.value)}
                    <tr><th scope="row">{rowLabel(t.kind, r.value)}</th><td class="num">{formatCount(r.visitors)}</td><td class="num">{formatCount(r.views)}</td></tr>
                  {/each}
                </tbody>
              </table>
            </section>
          {/if}
        {/each}
      {/if}
      {#if words}
        <section class="an-setting" aria-labelledby="an-setting">
          <h3 id="an-setting">{SETTING_HEADING}</h3>
          <p class="small muted" data-testid="analytics-setting-note">{words.note}</p>
          <button type="button" name="analytics" disabled={busy} onclick={() => { void toggle(); }}>{words.button}</button>
        </section>
      {/if}
      <div class="foot"><button type="button" {@attach focus} onclick={close}>Close</button></div>
    </div>
  </div>
{/if}

<style>
  .an-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter); }
  .an-panel { background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(640px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 18px 20px 20px; font-size: 13px; }
  .an-panel h2 { margin: 0 0 12px; font-size: 17px; overflow-wrap: anywhere; }
  .an-panel h3 { margin: 0 0 6px; font-size: 13px; font-weight: 600; }
  .an-panel .muted { color: var(--muted); }
  .an-panel .small { font-size: 12.5px; line-height: 1.5; margin: 6px 0 0; }
  .an-panel .err { color: var(--danger); }
  .an-range { display: inline-flex; flex-wrap: wrap; border: 1px solid var(--border-hover); border-radius: var(--radius-sm); overflow: hidden; }
  .an-range button { border: 0; border-radius: 0; min-height: 30px; padding: 4px 12px; background: transparent; }
  .an-range button + button { border-left: 1px solid var(--border-hover); }
  .an-range button[aria-pressed="true"] { background: var(--accent-tint); color: var(--accent-ink); font-weight: 600; }
  .an-empty { margin: 16px 0 0; font-size: 14px; }
  .an-totals { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 10px; margin: 16px 0 0; }
  .an-totals div { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 8px 10px; min-width: 0; }
  .an-totals dt { color: var(--muted); font-size: 12px; }
  .an-totals dd { margin: 2px 0 0; font-size: 20px; font-weight: 600; font-variant-numeric: tabular-nums; }
  .an-chart { margin: 16px 0 0; }
  .an-chart figcaption { display: flex; flex-wrap: wrap; gap: 4px 14px; align-items: center; font-weight: 600; margin-bottom: 8px; }
  .an-key { display: inline-flex; align-items: center; gap: 6px; font-weight: 400; color: var(--muted); font-size: 12px; }
  .sw { display: inline-block; width: 10px; height: 10px; border-radius: 2px; }
  .sw.views { background: var(--band); }
  .sw.visitors { background: var(--agent); }
  /* The chart's SVG is drawn from analytics-model.ts (chartSvg), so its parts are styled here by class. */
  .an-plot { position: relative; height: 140px; margin-top: 18px; }
  .an-draw { position: absolute; inset: 0; }
  .an-plot :global(svg) { display: block; width: 100%; height: 100%; overflow: visible; }
  .an-plot :global(rect.views) { fill: var(--band); }
  .an-plot :global(rect.visitors) { fill: var(--agent); }
  .an-plot :global(rect.hit) { fill: transparent; }
  .an-plot :global(g:hover rect.views) { fill: var(--border-strong); }
  .an-plot :global(line) { vector-effect: non-scaling-stroke; stroke-width: 1px; }
  .an-plot :global(line.grid) { stroke: var(--border); }
  .an-plot :global(line.base) { stroke: var(--border-strong); }
  .an-plot :global(line.mark) { stroke: var(--fg); stroke-dasharray: 3 3; }
  .an-max { position: absolute; right: 0; top: -16px; font-size: 11px; color: var(--muted); font-variant-numeric: tabular-nums; }
  .an-plot :global(.an-mark) { position: absolute; top: -16px; transform: translateX(-50%); font-size: 11px; color: var(--fg); background: var(--raised); padding: 0 3px; white-space: nowrap; }
  .an-axis { display: flex; justify-content: space-between; color: var(--muted); font-size: 11px; margin-top: 4px; }
  .an-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .an-table { margin-top: 16px; }
  .an-table table { width: 100%; border-collapse: collapse; table-layout: fixed; }
  .an-table th, .an-table td { padding: 5px 0; border-top: 1px solid var(--border); text-align: left; font-weight: 400; }
  .an-table thead th { border-top: 0; color: var(--muted); font-size: 12px; }
  .an-table tbody th { overflow-wrap: anywhere; padding-right: 8px; }
  .an-table .num { text-align: right; width: 88px; font-variant-numeric: tabular-nums; }
  .an-setting { margin-top: 16px; padding-top: 14px; border-top: 1px solid var(--border); }
  .an-setting button { margin-top: 8px; }
  .an-panel .foot { margin-top: 16px; display: flex; justify-content: flex-end; }
  @media (max-width: 700px) { .an-table .num { width: 64px; } .an-totals dd { font-size: 17px; } }
</style>
