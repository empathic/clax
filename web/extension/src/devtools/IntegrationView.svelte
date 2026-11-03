<script lang="ts">
  // The DevTools panel's Integration view (BRIEF-stint-2026-11-02-clax-script-
  // install Addendum 8): for the inspected page, which container and which
  // deployment put Clax there, the config version loaded, and every config
  // field with its value and where it came from (container, site default,
  // Clax default, or you). On a local or tailnet origin each field takes a
  // per-viewer override, kept in the page's cookie and applied at the next
  // load; elsewhere the rows are read-only. Choices the overlay has not
  // built show as "not built", never as a dead control. The page needs no
  // code of its own for any of this. What a change did is said in the
  // host's notices zone (`say`).
  import type { FieldName } from "../../../sdk/src/config";
  import { type Group, type Row, SOURCE_TEXT, groups, shownValue } from "./integration.svelte";
  import type { IntegrationLink } from "./integration.svelte";

  let { link, say }: { link: Pick<IntegrationLink, "status" | "info" | "set" | "refresh">; say(text: string, reload?: boolean): void } = $props();
  /** What the viewer chose here since the page loaded, by field: shown until the reload applies it. */
  let pending = $state<Partial<Record<FieldName, unknown>>>({});
  const all = $derived(link.info ? groups(link.info) : []);
  // A fresh read (the page reloaded) shows what applied: nothing is pending.
  $effect(() => { void link.info; pending = {}; });
  const fid = (r: Row) => `in-${r.field.replace(/\./g, "-")}`;

  async function set(r: Row, value: unknown): Promise<void> {
    const a = await link.set(r.field, value);
    if (!a.ok) {
      // The control shows the value kept again.
      const el = document.getElementById(fid(r));
      if (el instanceof HTMLInputElement && el.type === "checkbox") el.checked = r.value === true;
      for (const x of document.getElementsByName(fid(r))) if (x instanceof HTMLInputElement) x.checked = x.value === r.value;
      say(`${r.label} not changed: ${a.reason}`);
      return;
    }
    pending = { ...pending, [r.field]: value };
    say(value === null ? `${r.label}: your override dropped. Reload the page to apply.` : `${r.label}: ${shownValue({ ...r, value })} (You). Reload the page to apply.`, true);
  }
  const versionText = (v: number | null) =>
    v === null ? "none read" : v === 0 ? "0 (nothing published in Clax yet: the site's and Clax's defaults govern)" : String(v);
  const scriptText = (s: string | null) => {
    if (!s) return "not a tag install (npm, or a tag manager's queued install)";
    const pin = /[?&]v=([0-9a-f]{6,})/.exec(s)?.[1];
    return pin ? `pinned, version ${pin}` : "unpinned: follows each Clax deploy";
  };
  const groupId = (g: Group) => `in-g-${g.id}`;
</script>

<div class="integ">
  {#if link.status === "loading"}
    <p class="hint">Reading the page’s Clax install…</p>
  {:else if link.status === "none" || !link.info}
    <p class="hint">Clax is not installed on this page.</p>
    <p class="hint small">A site installs it with the tag from its queue’s Install section in Clax, or the Clax extension carries it.</p>
    <button type="button" class="ghost" onclick={() => link.refresh()}>Read again</button>
  {:else}
    {@const i = link.info}
    <section class="sect" aria-labelledby="in-where">
      <h2 id="in-where">Where it comes from</h2>
      <dl>
        <dt>Container</dt><dd class="mono">{i.container}</dd>
        <dt>Config version</dt><dd>{versionText(i.version)}</dd>
        <dt>Carrier</dt><dd>{i.carrier === "site" ? "The site’s own install" : "The Clax extension"}</dd>
        <dt>Deployment</dt><dd>{i.deployment ?? "not named (a tag from before deployments were named, or an npm install)"}</dd>
        <dt>Overlay</dt><dd>{scriptText(i.script)}</dd>
        <dt>Instance</dt><dd class="mono">{i.instance}</dd>
        <dt>Plugins</dt><dd>{i.plugins.length ? i.plugins.join(", ") : "none (Clax is off, or the extension takes reports)"}</dd>
      </dl>
      {#if i.error}<p class="warn" role="note">{i.error}</p>{/if}
      {#each i.ignored as x (`${x.layer}:${x.field}`)}
        <p class="desc">Ignored {SOURCE_TEXT[x.layer].toLowerCase()} value for <code>{x.field}</code>: {x.reason}.</p>
      {/each}
      <p class="desc">
        {#if i.applies}Your overrides are kept in this browser only, for {i.origin}, and apply when the page reloads.
        {:else}Read-only here: viewer overrides apply on local and tailnet origins only{#if i.config["overrides.viewer"] === "never"}, and this site turns them off{/if}.{/if}
      </p>
    </section>

    {#each all as g (g.id)}
      <section class="sect" aria-labelledby={groupId(g)}>
        <h2 id={groupId(g)}>{g.title}</h2>
        {#each g.rows as r (r.field)}
          {@const id = fid(r)}
          {@const mine = r.field in pending}
          <div class="field">
            {#if r.notBuilt}
              <p class="row"><span class="name">{r.label}</span> <span class="nb">Not built</span></p>
              <p class="desc">{r.notBuilt}</p>
            {:else if r.choices}
              <fieldset aria-describedby="{id}-src">
                <legend>{r.label}</legend>
                {#each r.choices as c (c.value)}
                  <div class="opt" class:off={!c.built}>
                    <input type="radio" id="{id}-{c.value}" name={id} value={c.value} checked={(mine ? pending[r.field] : r.value) === c.value}
                      disabled={!i.applies || !c.built} aria-describedby={c.built ? undefined : `${id}-${c.value}-nb`}
                      onchange={() => set(r, c.value)} />
                    <label for="{id}-{c.value}">{c.label}</label>
                    {#if !c.built}<small id="{id}-{c.value}-nb">Not built</small>{/if}
                  </div>
                {/each}
              </fieldset>
            {:else if r.kind === "bool"}
              <label class="toggle">
                <input {id} type="checkbox" role="switch" checked={(mine ? pending[r.field] : r.value) === true} disabled={!i.applies} aria-describedby="{id}-src"
                  onchange={e => set(r, e.currentTarget.checked)} />
                <span>{r.label}</span>
              </label>
            {:else if r.kind === "string"}
              <label class="name" for={id}>{r.label}</label>
              <input {id} type="text" maxlength={r.field === "entry.key" ? 1 : 200} disabled={!i.applies} aria-describedby="{id}-src"
                value={String(mine ? pending[r.field] : r.value)}
                onchange={e => set(r, e.currentTarget.value)} />
            {:else}
              <p class="row"><span class="name">{r.label}</span> {shownValue(r)}</p>
              {#if r.kind === "tokens" && Object.keys(r.value as object).length}
                <ul class="tokens">
                  {#each Object.entries(r.value as Record<string, string>) as [k, v] (k)}<li><code>{k}</code>: <code>{v}</code></li>{/each}
                </ul>
              {:else if r.kind === "targets" && (r.value as unknown[]).length}
                <ul class="tokens">
                  {#each r.value as { selector: string; action: string }[] as t (t.selector)}<li><code>{t.selector}</code> → {t.action}</li>{/each}
                </ul>
              {/if}
            {/if}
            {#if !r.notBuilt}
              <p class="src" id="{id}-src">
                From: {SOURCE_TEXT[r.source]}.
                {#if mine}You chose {pending[r.field] === null ? "to drop your override" : shownValue({ ...r, value: pending[r.field] })}: reload to apply.{/if}
              </p>
              {#if i.applies && (r.source === "you" || (mine && pending[r.field] !== null))}
                <button type="button" class="ghost" onclick={() => set(r, null)}>Reset {r.label}</button>
              {/if}
            {/if}
          </div>
        {/each}
      </section>
    {/each}

    <section class="sect" aria-labelledby="in-save">
      <h2 id="in-save">Save to the container</h2>
      <p class="row"><span class="nb">Not built</span></p>
      <p class="desc">Saving your overrides as the container’s next version needs stored container versions in Clax. Until then a viewer override is the way to try a setting here.</p>
    </section>
  {/if}
</div>

<style>
  .integ { padding: 8px var(--gutter) 18px; }
  .hint { color: var(--muted); margin: 12px 0 0; }
  .small { font-size: 12.5px; }
  .mono, code { font: 12px var(--mono); overflow-wrap: anywhere; }
  .sect { margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--border); }
  h2 { margin: 0; font-size: 14px; font-weight: 600; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 8px 0 0; font-size: 13px; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .desc { margin: 4px 0 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .warn { margin: 6px 0 0; font-size: 12.5px; }
  .field { margin-top: 12px; display: grid; gap: 4px; justify-items: start; }
  fieldset { margin: 0; padding: 0; border: 0; min-width: 0; display: grid; gap: 2px; }
  legend { padding: 0; font-weight: 500; margin-bottom: 4px; }
  .opt { display: grid; grid-template-columns: auto auto 1fr; column-gap: 8px; align-items: baseline; min-height: 24px; padding: 2px 0; }
  .opt small, .nb { color: var(--muted); font-size: 12px; }
  .nb { border: 1px solid var(--border); border-radius: 4px; padding: 0 4px; }
  .opt.off label { color: var(--muted); }
  .toggle { display: flex; gap: 8px; align-items: center; min-height: 24px; font-weight: 500; }
  .row { margin: 0; }
  .name { font-weight: 500; }
  input[type="text"] { width: 100%; max-width: 32rem; box-sizing: border-box; }
  .tokens { margin: 0; padding-left: 18px; font-size: 12.5px; }
  .src { margin: 0; font-size: 12px; color: var(--muted); }
  button.ghost { font-size: 12.5px; min-height: 24px; }
  @media (forced-colors: active) { .sect { border-color: CanvasText; } .nb { border-color: CanvasText; } }
</style>
