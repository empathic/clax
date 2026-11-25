<svelte:options css="injected" />

<script lang="ts">
  // Settings, Report queues: for the owner and the instance's
  // administrators, whether members who administer nothing may make report
  // queues (`GET/PUT /api/admin/queues`). A queue's service grant lets an
  // outside site file reports into this Clax, so an administrator decides
  // who may set one up; the default is on for a local Clax and off for a
  // hosted one. Off, only the owner and administrators make queues, and
  // every existing queue and its admins keep working. A refused change
  // says why and the switch shows what the daemon holds.
  import { onMount } from "svelte";
  import BotPage from "../bot/BotPage.svelte";
  import Switch from "../bot/Switch.svelte";
  import { refusal } from "../people/refusal";
  import { type QueuePolicy, getQueuePolicy, putQueuePolicy } from "../queues/api";
  import { holdReveal } from "./first-data";

  let policy = $state<QueuePolicy | null>(null);
  let err = $state<string | null>(null);
  let busy = $state(false);
  let saved = $state("");
  const done = holdReveal();

  async function set(on: boolean) {
    if (busy) return;
    busy = true; err = null; saved = "";
    try {
      policy = await putQueuePolicy(on);
      saved = on ? "Members may make report queues." : "Only the owner and administrators make report queues now.";
    } catch (x) { err = refusal(x); }
    busy = false;
  }

  onMount(() => {
    document.title = "Report queues · Settings · Clax";
    void getQueuePolicy().then(p => { policy = p; }, x => { err = `Could not load the setting: ${refusal(x)}`; }).finally(done);
  });
</script>

<BotPage title="Report queues" lede="Who may make the queues that collect reports filed from your sites' pages.">
  <section class="bcard" aria-labelledby="rq-h">
    <h3 id="rq-h">Members may make report queues</h3>
    {#if policy}
      <Switch checked={policy.members_create} label="Members may make report queues" hideLabel describedby="rq-d" {busy} onchange={on => void set(on)} />
      <p id="rq-d" class="note">
        {policy.members_create ? "On: anyone connected to this Clax may make a queue and its service grants." : "Off: only the owner and administrators make queues. Members can still triage the queues they're added to."}
        {#if policy.source === "default"} This is this Clax's default.{/if}
      </p>
    {:else if !err}
      <p class="note">Loading…</p>
    {/if}
    {#if err}<p class="err" role="alert">{err}</p>{/if}
    <p class="note" role="status">{saved}</p>
  </section>
</BotPage>
