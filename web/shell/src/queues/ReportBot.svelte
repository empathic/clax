<svelte:options css="injected" />

<script lang="ts">
  // ClaxBot on a report, in the report view's side column, loaded after the
  // report: Send to ClaxBot, its run at work or its last one, and the run
  // panel (`RunPanel`, where Stop is). Who may send is the rule of the
  // artifact view's offer (`BotOffer`): someone ClaxBot is available to who
  // may send the report's artifact's threads, as the daemon says
  // (`threadRuns`); nothing shows to anyone ClaxBot is not on for, or who has
  // no run to see and no right to send. A report
  // is sent while a fix may still be proposed on it, and not while someone
  // holds a claim on it, since ClaxBot's fix would then be refused. The runs
  // are read again whenever the report changes, and when a run shown changes
  // state.
  import { onDestroy, onMount, untrack } from "svelte";
  import type { Report } from "../../../sdk/src/types";
  import { type BotRun, type MeBot, type ThreadRun, threadRuns } from "../bot/api";
  import { myBot } from "../bot/me";
  import { credits, ended, runStatus, unavailableText } from "../bot/model";
  import { botView, sendToBot } from "../bot/send.svelte";
  import RunPanel from "../bot/RunPanel.svelte";
  import { noteOf } from "./model";

  const p: { report: Report } = $props();
  let me = $state<MeBot | null>(null);
  let runs = $state<ThreadRun[]>([]);
  let maySend = $state(false);
  let refused = $state<string | null>(null);
  let notice = $state<string | null>(null);

  const r = $derived(p.report);
  const active = $derived(runs.find(x => !ended(x)));
  const last = $derived(active ?? runs[0]);
  const taken = $derived(r.state === "claimed" && r.claim ? r.claim.holder_name : null);
  const open = $derived(r.state === "open" || r.state === "fix_proposed");
  const offered = $derived(!!me?.available && maySend && open && !active);
  const blocked = $derived(!!me && !me.available && maySend && open && !active);
  const cost = $derived(me?.estimate_milli ? `, usually ${credits(me.estimate_milli.low)}–${credits(me.estimate_milli.high)} credits` : "");
  const quotes = (ids: string[]) => (ids.includes(r.id) ? [{ id: r.id, text: noteOf(r).split("\n")[0].slice(0, 120) }] : []);
  // The daemon's word on who may stop a run; one sent from here is this viewer's.
  const mine = (x: BotRun) => botView.started.includes(x.id) || !!runs.find(y => y.id === x.id)?.may_stop;

  // Each read's turn: an answer after a later read is dropped.
  let turn = 0;
  async function load() {
    const at = ++turn;
    try {
      const got = await threadRuns(r.artifact.id, r.id);
      if (at !== turn) return;
      runs = got.runs; maySend = got.may_send;
    } catch {
      // The artifact is beyond this person's reach: no runs to show, and none to send.
      if (at === turn) { runs = []; maySend = false; }
    }
  }
  async function send() {
    refused = null; notice = null;
    let said: string | null = null;
    const started = await sendToBot({ id: r.artifact.id, setNotice(text) { said = text; } }, [r.id]);
    if (started) { notice = said; await load(); } else refused = said;
  }

  // Any change of the report, as this view hears it.
  $effect(() => { void p.report; if (me?.enabled) untrack(() => { void load(); }); });
  // The run shown changed state.
  let seen = botView.changes;
  $effect(() => { const n = botView.changes; if (n !== seen) { seen = n; untrack(() => { void load(); }); } });
  onMount(() => {
    botView.run = null; botView.started = [];
    void myBot().then(b => { me = b; botView.me = b; });
  });
  onDestroy(() => { botView.run = null; botView.started = []; });
</script>

{#if me?.enabled}
  {#if last || offered || blocked || (taken && maySend) || notice || refused}
    <section class="rbot" aria-label="ClaxBot">
      <h3>ClaxBot</h3>
      {#if active}
        <p>ClaxBot is working on this report.</p>
      {:else if last}
        <p class="muted">Last run: {runStatus(last)}</p>
      {/if}
      {#if offered}
        <p>ClaxBot can work on this report, using your credits{cost}.</p>
      {:else if blocked}
        <p class="muted">{unavailableText(me?.unavailable_reason, me?.week)}</p>
      {:else if taken && maySend && !active}
        <p class="muted">{taken} holds this report: release the claim to send it to ClaxBot.</p>
      {/if}
      {#if notice}<p class="muted" role="status">{notice}</p>{/if}
      {#if refused}<p class="err" role="alert">{refused}</p>{/if}
      {#if offered || last}
        <div class="row">
          {#if offered}<button type="button" class="primary" onclick={() => void send()}>Send to ClaxBot</button>{/if}
          {#if last}<button type="button" class="ghost" aria-controls="bot-run-panel" onclick={() => botView.open(last.id)}>See the run</button>{/if}
        </div>
      {/if}
    </section>
  {/if}
  <RunPanel aid={r.artifact.id} {runs} {quotes} {mine} />
{/if}

<style>
  .rbot p { margin: 0 0 6px; font-size: 13px; overflow-wrap: anywhere; }
  .rbot h3 { margin: 0 0 8px; font-size: 14px; }
  .muted { color: var(--muted); }
  .err { color: var(--danger); }
  .row { display: flex; flex-wrap: wrap; gap: 6px; }
  @media (pointer: coarse) { .row button { min-height: 44px; } }
</style>
