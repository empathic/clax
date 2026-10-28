<svelte:options css="injected" />

<script lang="ts">
  // A queue's form, new or editing: its name, the origins its reporters'
  // pages may file from (one per line) and what a reporter reads, their
  // own reports or every report of the queue. A line that is not an
  // origin is named, and nothing is sent until it is fixed. `onSave`
  // answers the refusal's message, or null once saved.
  import { onMount } from "svelte";
  import type { Queue, QueueFields, ReportersSee } from "./api";
  import { parseOrigins } from "./model";

  const p: { queue?: Queue; onSave(f: QueueFields): Promise<string | null>; onCancel(): void } = $props();
  // svelte-ignore state_referenced_locally
  let name = $state(p.queue?.name ?? "");
  // svelte-ignore state_referenced_locally
  let origins = $state(p.queue?.origins.join("\n") ?? "");
  // svelte-ignore state_referenced_locally
  let see = $state<ReportersSee>(p.queue?.reporters_see ?? "own");
  let err = $state<string | null>(null);
  let busy = $state(false);
  const parsed = $derived(parseOrigins(origins));
  // svelte-ignore state_referenced_locally
  const key = p.queue?.id ?? "new";

  async function submit(e: Event) {
    e.preventDefault();
    if (busy) return;
    if (parsed.bad.length) { err = `Not an origin: ${parsed.bad.join(", ")}. Write each as https://host, or http://host:port, with no path.`; return; }
    busy = true; err = null;
    try { err = await p.onSave({ name: name.trim(), origins: parsed.origins, reporters_see: see }); } finally { busy = false; }
  }
  onMount(() => { document.querySelector<HTMLElement>(`form.qform[data-key="${key}"] input[name="name"]`)?.focus(); });
</script>

<form class="qform" data-key={key} onsubmit={submit}>
  <label class="field">Name <input name="name" autocomplete="off" required maxlength="80" value={name} oninput={e => { name = e.currentTarget.value; }} /></label>
  <label class="field">Origins, one per line
    <textarea name="origins" rows="3" spellcheck="false" placeholder="https://app.example" value={origins} oninput={e => { origins = e.currentTarget.value; }}></textarea>
    <span class="hint">The sites whose pages may file reports into this queue.</span>
  </label>
  <fieldset class="see">
    <legend>A reporter sees</legend>
    <label><input type="radio" name={`see-${key}`} value="own" checked={see === "own"} onchange={() => { see = "own"; }} /> Their own reports</label>
    <label><input type="radio" name={`see-${key}`} value="all" checked={see === "all"} onchange={() => { see = "all"; }} /> Every report in the queue</label>
  </fieldset>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  <div class="row">
    <button class="primary" type="submit" disabled={!name.trim()}>{p.queue ? "Save" : "Create queue"}</button>
    <button class="ghost" type="button" onclick={p.onCancel}>Cancel</button>
  </div>
</form>

<style>
  .qform { display: flex; flex-direction: column; gap: 12px; padding: 14px 16px; margin: 0 0 12px; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); min-width: 0; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); }
  .field input { width: 320px; max-width: 100%; }
  .field textarea { width: 100%; max-width: 480px; box-sizing: border-box; font: 13px/1.5 var(--mono); color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 8px; resize: vertical; }
  .hint { font-size: 12px; }
  .see { display: flex; flex-wrap: wrap; gap: 6px 18px; margin: 0; padding: 0; border: 0; }
  .see legend { width: 100%; padding: 0 0 4px; font-size: 12.5px; color: var(--muted); }
  .see label { display: inline-flex; align-items: center; gap: 6px; font-size: 14px; }
  .err { margin: 0; color: var(--danger); font-size: 13px; }
  .row { display: flex; flex-wrap: wrap; gap: 8px; }
</style>
