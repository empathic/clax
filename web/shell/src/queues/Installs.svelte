<svelte:options css="injected" />

<script lang="ts">
  // A queue's installs: each site that embeds the overlay and files into
  // the queue, with what its overlay is configured with. Every member
  // reads the list; an admin makes, edits and deletes them. A host page
  // carries only an install's ID (the meta tag shown with each) and its
  // overlay fetches the rest from Clax, so an edit here reaches the site
  // on its next page load.
  import { onMount } from "svelte";
  import * as api from "./api";
  import { parseOrigins, refusal } from "./model";

  const p: { queue: api.Queue; admin: boolean } = $props();
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;
  let installs = $state<api.Install[] | null>(null);
  let err = $state<string | null>(null);
  /** The install being edited ("new" for the form that makes one). */
  let editing = $state<string | null>(null);
  let copied = $state<string | null>(null);

  type Form = { name: string; origins: string; product_name: string; report_label: string; queue_label: string; reports_button: boolean; picker: boolean; brand_color: string };
  const blank = (): Form => ({ name: "", origins: p.queue.origins.join("\n"), product_name: "", report_label: "", queue_label: "", reports_button: true, picker: true, brand_color: "" });
  const formOf = (i: api.Install): Form => ({
    name: i.name, origins: i.origins.join("\n"),
    product_name: i.config.product_name ?? "", report_label: i.config.report_label ?? "", queue_label: i.config.queue_label ?? "",
    reports_button: i.config.reports_button, picker: i.config.picker, brand_color: i.config.brand_color ?? "",
  });
  let form = $state<Form>(blank());

  const load = () => api.listInstalls(qid).then(b => { installs = b.installs; }, x => { err = `Could not load the installs: ${refusal(x)}`; });
  const opt = (s: string) => s.trim() || null;

  function open(id: string | null, i?: api.Install) {
    err = null;
    editing = id;
    form = i ? formOf(i) : blank();
  }

  async function save(e: Event) {
    e.preventDefault();
    const { origins, bad } = parseOrigins(form.origins);
    if (bad.length) { err = `Not an origin: ${bad.join(", ")}`; return; }
    const fields: api.InstallFields = {
      name: form.name.trim(), origins,
      config: { product_name: opt(form.product_name), report_label: opt(form.report_label), queue_label: opt(form.queue_label), reports_button: form.reports_button, picker: form.picker, brand_color: opt(form.brand_color) },
    };
    err = null;
    try {
      if (editing === "new") await api.createInstall(qid, fields);
      else if (editing) await api.updateInstall(qid, editing, fields);
      editing = null;
    } catch (x) { err = refusal(x); }
    await load();
  }

  async function remove(i: api.Install) {
    if (!confirm(`Delete the install "${i.name}"? Its site's overlay falls back to its page's own settings.`)) return;
    err = null;
    try { await api.deleteInstall(qid, i.id); } catch (x) { err = refusal(x); }
    if (editing === i.id) editing = null;
    await load();
  }

  const metaTag = (id: string) => `<meta name="clax-install" content="${id}">`;
  const copy = (id: string) => { void navigator.clipboard?.writeText(metaTag(id)).then(() => { copied = id; }, () => {}); };
  const summary = (c: api.InstallConfig) => [
    c.product_name ? `named "${c.product_name}"` : "no product name",
    c.reports_button ? "Your reports" : "no Your reports",
    c.picker ? "element picker" : "no element picker",
  ].join(" · ");

  onMount(() => { void load(); });
</script>

{#snippet editor(label: string)}
  <form class="edit" aria-label={label} onsubmit={save}>
    <label class="field">Name <input name="install_name" autocomplete="off" maxlength="120" placeholder="Changelist" value={form.name} oninput={e => { form.name = e.currentTarget.value; }} /></label>
    <label class="field">Origins, one per line <textarea name="install_origins" rows="2" spellcheck="false" value={form.origins} oninput={e => { form.origins = e.currentTarget.value; }}></textarea></label>
    <label class="field">Product name <input name="product_name" maxlength="80" placeholder="none" value={form.product_name} oninput={e => { form.product_name = e.currentTarget.value; }} /></label>
    <label class="field">Report button label <input name="report_label" maxlength="80" placeholder="Report" value={form.report_label} oninput={e => { form.report_label = e.currentTarget.value; }} /></label>
    <label class="field">Words before the queue's name <input name="queue_label" maxlength="80" placeholder="Files into" value={form.queue_label} oninput={e => { form.queue_label = e.currentTarget.value; }} /></label>
    <label class="field">Accent colour <input name="brand_color" maxlength="7" placeholder="#rrggbb" spellcheck="false" value={form.brand_color} oninput={e => { form.brand_color = e.currentTarget.value; }} /></label>
    <label class="check"><input type="checkbox" name="reports_button" checked={form.reports_button} onchange={e => { form.reports_button = e.currentTarget.checked; }} /> Show "Your reports"</label>
    <label class="check"><input type="checkbox" name="picker" checked={form.picker} onchange={e => { form.picker = e.currentTarget.checked; }} /> Offer the element picker</label>
    <div class="row">
      <button type="submit" class="primary" disabled={!form.name.trim()}>{editing === "new" ? "Add install" : "Save"}</button>
      <button type="button" class="ghost" onclick={() => { editing = null; err = null; }}>Cancel</button>
    </div>
  </form>
{/snippet}

<section class="part" aria-label={`Installs of ${p.queue.name}`}>
  <h4>Installs</h4>
  <p class="hint">Each site that files into this queue. Its page carries only the install's ID; its overlay fetches the rest from here.</p>
  {#if installs?.length}
    <ul class="lines">
      {#each installs as i (i.id)}
        <li class="line" data-install={i.id}>
          <div class="who">
            {i.name}<span class="key">{i.origins.join(", ") || "no origins"}</span>
            <span class="sum">{summary(i.config)}</span>
            <span class="cmd"><code>{metaTag(i.id)}</code><button type="button" onclick={() => copy(i.id)}>{copied === i.id ? "Copied" : "Copy"}</button></span>
          </div>
          {#if p.admin}
            <button type="button" class="ghost" aria-expanded={editing === i.id} onclick={() => open(editing === i.id ? null : i.id, i)}>Edit</button>
            <button type="button" class="ghost danger" onclick={() => void remove(i)}>Delete</button>
          {/if}
        </li>
        {#if editing === i.id}{@render editor(`Edit ${i.name}`)}{/if}
      {/each}
    </ul>
  {:else if installs}
    <p class="none">No installs yet.</p>
  {/if}
  {#if p.admin}
    {#if editing === "new"}{@render editor("New install")}
    {:else}<button type="button" onclick={() => open("new")}>New install</button>{/if}
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</section>

<style>
  .part { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border); min-width: 0; }
  h4 { margin: 0 0 6px; font-size: 12.5px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .lines { list-style: none; margin: 0 0 8px; padding: 0; }
  .line { display: flex; flex-wrap: wrap; align-items: start; gap: 4px 10px; min-width: 0; padding: 6px 0; }
  .line + .line { border-top: 1px dashed var(--border); }
  .who { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; font-weight: 600; font-size: 14px; overflow-wrap: anywhere; }
  .key, .sum { font: 400 12px var(--mono); color: var(--muted); }
  .sum { font-family: inherit; }
  .cmd { display: flex; gap: 8px; align-items: stretch; min-width: 0; font-weight: 400; }
  .cmd code { flex: 1; min-width: 0; padding: 4px 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); font: 12px/1.4 var(--mono); overflow-wrap: anywhere; user-select: all; }
  .edit { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 8px 12px; margin: 6px 0 12px; padding: 12px; border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); min-width: 0; }
  .field input, .field textarea { max-width: 100%; }
  .check { display: flex; gap: 6px; align-items: center; font-size: 13px; }
  .row { grid-column: 1 / -1; display: flex; gap: 8px; }
  .hint, .none { margin: 6px 0; color: var(--muted); font-size: 12.5px; }
  .err { margin: 6px 0 0; color: var(--danger); font-size: 13px; }
  button.danger { color: var(--danger); }
</style>
