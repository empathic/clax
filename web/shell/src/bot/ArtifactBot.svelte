<svelte:options css="injected" />

<script lang="ts">
  // ClaxBot in the artifact view's top bar (spec 2026-10-21-claxbot §11.2–4),
  // loaded with the roster after the first paint (`Who.svelte`), and only
  // where ClaxBot is on for this browser or the address names a run. It
  // lists ClaxBot to the Send menu (`botView`): as the artifact's agents list
  // it (`builtin`), else as this person's ClaxBot says. It shows a ClaxBot
  // button while the artifact has runs (marked while one is active) that
  // opens the run panel, the week's meter beside it, and, from 80 % of the
  // week's credits, the usage banner under the top bar. The panel opens by
  // itself on a run this browser starts, and on `#run=<ID>` (or `#run`, the
  // newest active run) in the address, which the New dialog and the audit
  // log link with; a run the artifact's list does not hold is read on its
  // own, once. Each finished step reads this person's ClaxBot again, so the
  // meter moves.
  import { fromStore } from "svelte/store";
  import { mount, onMount, unmount, untrack } from "svelte";
  import type { ArtifactController } from "../view/artifact-controller";
  import { type BotRun, type RunSummary, getRun, listRuns } from "./api";
  import { myBot, refresh } from "./me";
  import { credits, ended, meter } from "./model";
  import { botView } from "./send.svelte";
  import RunPanel from "./RunPanel.svelte";
  import UsageBanner from "./UsageBanner.svelte";

  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  let runs = $state<RunSummary[]>([]);
  const active = $derived(runs.some(r => !ended(r)));
  const week = $derived(botView.me?.enabled ? meter(botView.me.week) : null);
  const shown = $derived(runs.length > 0 || botView.started.length > 0);
  // The first lines of the threads a run answers, as the view holds them.
  const quotes = (ids: string[]) => ids.flatMap(id => {
    const t = view.current.threads.find(x => x.id === id);
    const body = t?.comments?.[0]?.body;
    return body ? [{ id, text: body.split("\n")[0].slice(0, 120) }] : [];
  });
  const onThread = (id: string) => { const t = view.current.threads.find(x => x.id === id); if (t) ctl.selectThread(t); };
  const onStep = () => { void refresh().then(b => { if (b) botView.me = b; }); };
  // Whether a run acts for this viewer, who may stop it; anyone else's is read-only.
  const mine = (r: BotRun) => {
    const me = view.current.me;
    const who = r.acting_for?.id;
    return !who || botView.started.includes(r.id) || (!!me && (who === me.public_id || (me.owner === true && who === "*owner")));
  };
  const panel = $state({ aid: "", runs: [] as RunSummary[], quotes, onThread, onStep, mine });
  $effect(() => { panel.runs = runs; });
  const sorted = (l: RunSummary[]) => [...l].sort((a, b) => b.started_at.localeCompare(a.started_at));
  const load = () => listRuns(ctl.id).then(r => { runs = sorted(Array.isArray(r?.runs) ? r.runs : []); return runs; }, () => runs);
  // The run shown changed state: read the list again.
  let seen = botView.changes;
  $effect(() => { const n = botView.changes; if (n !== seen) { seen = n; untrack(() => { void load(); }); } });
  // A run shown that the list does not hold (just started, another
  // artifact's, one the list leaves out): read on its own, once per ID.
  const tried = new Set<string>();
  $effect(() => {
    const id = botView.run;
    if (!id || tried.has(id) || untrack(() => runs.some(r => r.id === id))) return;
    tried.add(id);
    void getRun(id).then(r => { const { steps: _steps, ...summary } = r; if (!runs.some(x => x.id === id)) runs = sorted([...runs, summary]); }, () => {});
  });
  // ClaxBot as the Send menu lists it: from the artifact's agents when they hold it.
  $effect(() => {
    const listed = view.current.agents.find(a => a.builtin);
    const me = botView.me;
    untrack(() => {
      if (listed) botView.list(ctl.id, listed.live, listed.unavailable ?? null, me);
      else if (me?.enabled) botView.list(ctl.id, me.available, me.unavailable_reason ?? null, me);
    });
  });

  onMount(() => {
    const aid = ctl.id;
    panel.aid = aid;
    botView.run = null;
    botView.started = [];
    const asked = /^#run(?:=(.+))?$/.exec(location.hash);
    let banner: Record<string, unknown> | null = null;
    let bannerAt: HTMLElement | null = null;
    void myBot().then(me => {
      botView.me = me;
      if (!me?.enabled && !asked && !view.current.agents.some(a => a.builtin)) return;
      // The usage banner under the top bar, from 80 % of the week's credits.
      const bar = document.querySelector(".page > .topbar");
      if (me?.enabled && bar) {
        bannerAt = document.createElement("div");
        bannerAt.className = "bot-banner-at";
        bar.after(bannerAt);
        banner = mount(UsageBanner, { target: bannerAt, props: { me } });
      }
      void load().then(list => {
        if (!asked) return;
        const id = asked[1] ? decodeURIComponent(asked[1]) : (list.find(r => !ended(r)) ?? list[0])?.id;
        if (id) botView.open(id);
      });
    });
    const target = document.createElement("div");
    document.body.append(target);
    const v = mount(RunPanel, { target, props: panel });
    return () => {
      void unmount(v); target.remove();
      if (banner) void unmount(banner);
      bannerAt?.remove();
      botView.agent = null; botView.run = null;
    };
  });
</script>

{#if shown}
  <button type="button" class="botbtn" class:on={active} aria-expanded={!!botView.run} aria-controls="bot-run-panel"
    onclick={() => { if (botView.run) botView.close(); else { const id = (runs.find(r => !ended(r)) ?? runs[0])?.id ?? botView.started[0]; if (id) botView.open(id); } }}>
    <span class="dot" aria-hidden="true"></span>ClaxBot<span class="bot-sr">{active ? ", working" : ", runs"}</span>
  </button>
{/if}
{#if week}<a class="botwk hide-sm" class:warn={week.warn} class:out={week.exhausted} href="/me" title="Your ClaxBot credits this week">{credits(week.spent)} / {credits(week.total)}</a>{/if}

<style>
  :global {
    .topbar .botbtn { flex: none; gap: 6px; font-weight: 600; }
    .botbtn .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--border-strong); }
    .botbtn.on .dot { background: var(--agent); }
    .botbtn.on { color: var(--agent-ink); }
    .topbar .botbtn[aria-expanded="true"] { background: var(--hover); }
    .botwk { flex: none; font: 500 12.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; padding: 0 4px; }
    .botwk.warn { color: var(--tag-yellow-fg); }
    .botwk.out { color: var(--danger); }
    .bot-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .bot-banner-at { flex: none; }
    .bot-banner-at .bbanner { margin: 0; border-radius: 0; border-width: 0 0 1px; padding: 8px var(--gutter); }
  }
</style>
