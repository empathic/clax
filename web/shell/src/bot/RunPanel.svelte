<svelte:options css="injected" />

<script lang="ts">
  // The run panel (spec 2026-10-21-claxbot §11.3), beside the artifact (a
  // sheet at phone width): one ClaxBot run, live. Its narration, each script
  // as a card (its title, its code, every `clax` call it made with how it
  // went and how long it took, and its output), collapsed except the live
  // one and any that failed; the credits the run has spent and the week's
  // meter; Stop while it runs; a link to its events in the audit log; and,
  // once it stopped with an unpublished draft, Open the draft. Other runs of
  // the artifact are a choice away. Its head names the threads the run
  // answers (each one's first line, which shows it in the view), and a
  // polite live line says each step as it finishes. Escape or Close closes
  // it, and the focus goes back to what opened it.
  import { onDestroy } from "svelte";
  import { refusal } from "../people/refusal";
  import { type BotRun, type BotStep, type RunEvent, type RunSummary, cancelRun } from "./api";
  import { credits, ended, meter, openByDefault, runCost, runStatus, stepLine } from "./model";
  import { RunFeed } from "./run-feed.svelte";
  import { botView } from "./send.svelte";
  import Meter from "./Meter.svelte";

  type Hear = (ev: RunEvent, r: BotRun) => void;
  let { aid, runs, quotes = () => [], onThread, onStep, mine = () => true, feedFor = (id: string, h: Hear) => new RunFeed(id, undefined, h) }: {
    aid: string; runs: RunSummary[];
    /** The first lines of threads `ids`, as the view holds them. */
    quotes?: (ids: string[]) => { id: string; text: string }[];
    /** Shows thread `id` in the view. */
    onThread?: (id: string) => void;
    /** Hears each finished step and the run's end. */
    onStep?: Hear;
    /** Whether the run acts for this viewer, who may stop it; anyone else sees it read-only. */
    mine?: (r: BotRun) => boolean;
    feedFor?: (id: string, h: Hear) => RunFeed;
  } = $props();
  // The live line: the last step finished, in words.
  let said = $state("");
  const hear: Hear = (ev, r) => { said = stepLine(ev, r); onStep?.(ev, r); };
  let feed = $state<RunFeed | null>(null);
  let stopping = $state(false);
  let err = $state<string | null>(null);
  let panel: HTMLElement | undefined = $state();
  let head: HTMLElement | undefined = $state();
  // The runs' select shows the run shown (set here: options come and go).
  let pick: HTMLSelectElement | undefined = $state();
  $effect(() => { if (pick && botView.run && choices.length) pick.value = botView.run; });
  // The heading takes the focus as the panel opens on a run.
  $effect(() => { if (botView.run) head?.focus({ preventScroll: true }); });
  let back: HTMLElement | null = null;
  // One feed per run shown: a new run starts a new one.
  $effect(() => {
    const id = botView.run;
    if (!id) { feed = null; return; }
    back ??= document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const f = feedFor(id, hear);
    feed = f;
    stopping = false; err = null;
    f.start();
    return () => f.stop();
  });
  const run = $derived(feed?.run ?? null);
  // Each change of the run's state is told, for the runs listed beside it.
  let lastStatus = "";
  $effect(() => { const st = run?.status; if (st && st !== lastStatus) { lastStatus = st; botView.changes++; } });
  const live = $derived(!!run && !ended(run));
  const week = $derived(botView.me?.enabled ? meter(botView.me.week) : null);
  const choices = $derived([...new Set([...(botView.run ? [botView.run] : []), ...botView.started, ...runs.map(r => r.id)])]);
  const label = (id: string) => { const r = runs.find(x => x.id === id); return r ? `${new Date(r.started_at).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })} · ${runStatus(r)}` : "This run"; };
  const what = $derived(!run ? "" : run.request === "brief" ? "Building from the brief" : run.thread_ids.length === 1 ? "Answering one thread" : `Answering ${run.thread_ids.length} threads`);
  const answers = $derived(run ? quotes(run.thread_ids) : []);
  const callsLine = (s: BotStep) => { const c = s.calls ?? []; const ms = c.reduce((n, x) => n + x.ms, 0); return s.status === "running" ? `running · ${c.length} call${c.length === 1 ? "" : "s"}` : `${c.length} call${c.length === 1 ? "" : "s"} · ${ms} ms`; };
  const output = (s: BotStep) => (s.value === undefined ? null : typeof s.value === "string" ? s.value : JSON.stringify(s.value, null, 2));
  async function stop() {
    if (!run || stopping) return;
    stopping = true; err = null;
    try { await cancelRun(run.id); } catch (e) { err = refusal(e); stopping = false; }
  }
  function close() { botView.close(); back?.focus?.(); back = null; }
  const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); close(); } };
  onDestroy(() => feed?.stop());
</script>

{#if botView.run}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <aside class="runp" id="bot-run-panel" aria-labelledby="runp-h" bind:this={panel} onkeydown={key}>
    <header>
      <h2 id="runp-h" tabindex="-1" bind:this={head}>ClaxBot</h2>
      {#if run}<span class="st" class:on={live} role="status">{runStatus(run)}</span>{/if}
      <span class="sp"></span>
      {#if live && run && mine(run)}<button type="button" class="stop" disabled={stopping || run?.status === "stopping"} onclick={stop}>{stopping || run?.status === "stopping" ? "Stopping…" : "Stop"}</button>{/if}
      <button type="button" class="ghost" aria-label="Close the run panel" onclick={close}>Close</button>
    </header>
    {#if choices.length > 1}
      <label class="pick"><span>Run</span>
        <select bind:this={pick} onchange={e => botView.open(e.currentTarget.value)}>{#each choices as id (id)}<option value={id} selected={id === botView.run}>{label(id)}</option>{/each}</select>
      </label>
    {/if}
    {#if run}<p class="what">{what} · {run.model}{run.acting_for?.name ? ` · for ${run.acting_for.name}` : ""}</p>{/if}
    {#if run && !mine(run)}<p class="what">{run.acting_for?.name ?? "Someone else"}'s run: you can follow it, and only they can stop it.</p>{/if}
    {#if answers.length}
      <ul class="answers" aria-label="Threads it answers">
        {#each answers as q (q.id)}<li>{#if onThread}<button type="button" class="ghost" onclick={() => onThread(q.id)}>“{q.text}”</button>{:else}“{q.text}”{/if}</li>{/each}
      </ul>
    {/if}
    <p class="bot-sr" aria-live="polite">{said}</p>
    {#if err}<p class="err" role="alert">{err}</p>{/if}
    {#if feed?.error}<p class="err" role="alert">{feed.error}</p>{/if}
    <ol class="steps" aria-label="Steps">
      {#each run?.steps ?? [] as s (s.n)}
        <li class={`step ${s.kind}`}>
          {#if s.kind === "turn"}
            {#if s.text}<p class="who">ClaxBot</p><p class="say">{s.text}</p>{:else if s.status === "running"}<p class="say muted">Thinking…</p>{/if}
          {:else if s.kind === "script"}
            <details open={openByDefault(s) || !!s.error}>
              <summary><span class="t">{s.title ?? "Script"}</span><span class="meta">{callsLine(s)}</span>{#if s.error}<span class="bad">failed</span>{/if}</summary>
              {#if s.code}<pre class="code" aria-label="Code"><code>{s.code}</code></pre>{/if}
              {#if s.calls?.length}
                <ul class="calls" aria-label="Calls">
                  {#each s.calls as c, i (i)}<li class:bad={!c.ok}><span aria-label={c.ok ? "ok" : "failed"}>{c.ok ? "✓" : "✗"}</span> <code>{c.op}</code>{#if c.target}{" "}<span class="tg">{c.target}</span>{/if}<span class="ms">{c.ms} ms</span>{#if c.error}<span class="ce">{c.error}</span>{/if}</li>{/each}
                </ul>
              {/if}
              {#if s.logs?.length}<pre class="out" aria-label="Logs">{s.logs.join("\n")}</pre>{/if}
              {#if output(s) !== null}<pre class="out" aria-label="Output">{output(s)}</pre>{/if}
              {#if s.error}<p class="err">{s.error}</p>{/if}
            </details>
          {:else if s.kind === "finish"}
            <p class="who">Done</p>{#if s.summary}<p class="say">{s.summary}</p>{/if}
          {:else if s.kind === "give_up"}
            <p class="who">Gave up</p>{#if s.reason}<p class="say">{s.reason}</p>{/if}
          {:else}
            <p class="who">Stopped</p>{#if s.reason}<p class="say">{s.reason}</p>{/if}
          {/if}
        </li>
      {:else}
        {#if !feed?.error}<li class="step"><p class="say muted">{run ? "Starting…" : "Loading the run…"}</p></li>{/if}
      {/each}
    </ol>
    {#if run?.draft && run.status === "stopped"}
      <p class="draft">Its changes so far are saved as a draft, not published. <a href={run.draft.url}>Open the draft</a></p>
    {/if}
    <footer>
      {#if run}<p class="cost">{runCost(run)}</p>{/if}
      <div class="wk">
        {#if week}<Meter m={week} small label="Your credits this week" /><span>{credits(week.spent)} / {credits(week.total)}</span>{/if}
        <a href={`/audit?artifact=${encodeURIComponent(aid)}${botView.run ? `&q=${encodeURIComponent(botView.run)}` : ""}`}>Audit ↗</a>
      </div>
    </footer>
  </aside>
{/if}

<style>
  :global {
    .runp { position: fixed; z-index: 30; top: 60px; right: 0; bottom: 0; width: min(420px, 100vw); display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; background: var(--raised); color: var(--fg); border-left: 1px solid var(--border-hover); box-shadow: var(--elev-lg); overflow-y: auto; overscroll-behavior: contain; font-size: 13.5px; }
    .runp header { display: flex; align-items: center; gap: 8px; }
    .runp h2 { margin: 0; font-size: 16px; }
    .runp h2:focus { outline: none; }
    .runp .sp { flex: 1; }
    .runp .st { font-size: 12px; color: var(--muted); }
    .runp .st.on { color: var(--agent-ink); font-weight: 600; }
    .runp .st.on::before { content: ""; display: inline-block; width: 7px; height: 7px; border-radius: 50%; background: var(--agent); margin-right: 6px; vertical-align: 1px; }
    .runp .stop { color: var(--danger); }
    .runp .pick { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--muted); }
    .runp .pick select { flex: 1; min-width: 0; }
    .runp .what { margin: 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .runp .answers { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
    .runp .answers li { font-size: 13px; overflow-wrap: anywhere; }
    .runp .answers button { width: 100%; justify-content: flex-start; text-align: start; white-space: normal; min-height: 28px; padding: 4px 6px; font-weight: 400; }
    .runp .bot-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .runp .steps { list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; }
    .runp .who { margin: 0; font: 600 12px var(--font); color: var(--agent-ink); }
    .runp .say { margin: 2px 0 0; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
    .runp .muted { color: var(--muted); }
    .runp details { border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--card); }
    .runp summary { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; padding: 8px 10px; cursor: pointer; }
    .runp summary .t { font-weight: 600; overflow-wrap: anywhere; min-width: 0; }
    .runp summary .meta { font-size: 12px; color: var(--muted); margin-left: auto; }
    .runp .bad { color: var(--danger); }
    .runp summary .bad { font-size: 12px; font-weight: 600; }
    .runp pre { margin: 0 10px 8px; font: 12px/1.5 var(--mono); background: var(--hover); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 6px 8px; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 260px; overflow: auto; }
    .runp .calls { list-style: none; margin: 0 10px 8px; padding: 0; display: grid; gap: 2px; font-size: 12.5px; }
    .runp .calls li { display: flex; flex-wrap: wrap; gap: 0 6px; align-items: baseline; }
    .runp .calls li > span:first-child { color: var(--agent-ink); }
    .runp .calls li.bad > span:first-child { color: var(--danger); }
    .runp .calls code { font: 12px var(--mono); }
    .runp .calls .tg { color: var(--muted); overflow-wrap: anywhere; min-width: 0; }
    .runp .calls .ms { margin-left: auto; color: var(--muted); font-variant-numeric: tabular-nums; }
    .runp .calls .ce { flex-basis: 100%; color: var(--danger); }
    .runp details .err { margin: 0 10px 8px; }
    .runp .err { margin: 0; color: var(--danger); font-size: 12.5px; }
    .runp .draft { margin: 0; padding: 8px 10px; border-radius: var(--radius-sm); background: var(--accent-tint); }
    .runp .draft a, .runp footer a { color: var(--accent-ink); font-weight: 600; text-decoration: underline; }
    .runp footer { margin-top: auto; padding-top: 8px; border-top: 1px solid var(--border); display: grid; gap: 6px; }
    .runp .cost { margin: 0; font-size: 12.5px; color: var(--muted); }
    .runp .wk { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--muted); font-variant-numeric: tabular-nums; }
    .runp .wk .bmeter { flex: 1; }
    .runp .wk a { margin-left: auto; }
    @media (max-width: 700px) { .runp { top: 56px; width: 100vw; border-left: 0; padding: 12px var(--gutter); } }
  }
</style>
