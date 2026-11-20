<svelte:options css="injected" />

<script lang="ts">
  // Settings' Queues control for one agent token (and the Create token
  // form): a one-line summary that opens to the queues the token may touch
  // (All my queues, or some by name) and the actions there (Read, Claim,
  // Reply, Fix, Resolve; each includes Read). Every change is handed to
  // `onchange` at once, which sends it (or, in the Create token form,
  // keeps it for the new token). Queue names are people's text, shown as
  // text.
  import {
    ACTION_HINTS, ACTION_LABELS, QUEUE_ACTIONS, summary, withAction, withQueue,
    type QueueAction, type QueueChoice,
  } from "./queue-access";

  let { choice, queues, label, onchange, disabled = false }: {
    choice: QueueChoice;
    queues: { id: string; name: string }[];
    label: string;
    onchange: (c: QueueChoice) => void;
    disabled?: boolean;
  } = $props();
  const names = $derived(Object.fromEntries(queues.map(q => [q.id, q.name])));
  const line = $derived(summary(choice, names));
  const setAction = (a: QueueAction, on: boolean) => onchange(withAction(choice, a, on));
  const setQueue = (id: string, on: boolean) => onchange(withQueue(choice, id, on));
</script>

<details class="qaccess">
  <summary aria-label="Queue access for {label}: {line}"><span class="qlabel">Queues</span> <span class="qline">{line}</span></summary>
  {#if queues.length === 0}
    <p class="none">You are a member of no report queue, so there is nothing to grant.</p>
  {:else}
    <fieldset class="qwhere" {disabled}>
      <legend>Queues</legend>
      <label><input type="checkbox" name="all" checked={choice.all} onchange={e => setQueue("*", e.currentTarget.checked)} /> All my queues</label>
      {#each queues as q (q.id)}
        <label><input type="checkbox" name="queue" value={q.id} checked={choice.all || choice.queues.includes(q.id)} disabled={choice.all} onchange={e => setQueue(q.id, e.currentTarget.checked)} /> {q.name}</label>
      {/each}
    </fieldset>
    <fieldset class="qwhat" {disabled}>
      <legend>May</legend>
      {#each QUEUE_ACTIONS as a (a)}
        <label title={ACTION_HINTS[a]}><input type="checkbox" name="action" value={a} checked={choice.actions.includes(a)} onchange={e => setAction(a, e.currentTarget.checked)} /> {ACTION_LABELS[a]}</label>
      {/each}
    </fieldset>
  {/if}
</details>

<style>
  :global {
    .qaccess { margin: 4px 0 2px; font-size: 13px; }
    .qaccess summary { cursor: pointer; color: var(--muted); list-style-position: inside; }
    .qaccess summary .qlabel { color: var(--fg); font-weight: 500; }
    .qaccess fieldset { display: flex; flex-wrap: wrap; gap: 4px 14px; margin: 8px 0 0; padding: 6px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); min-width: 0; }
    .qaccess legend { padding: 0 4px; font-size: 12px; color: var(--muted); }
    .qaccess label { display: inline-flex; align-items: center; gap: 5px; overflow-wrap: anywhere; }
    .qaccess .none { margin: 6px 0 0; color: var(--muted); }
  }
</style>
