<script lang="ts">
  // The DevTools panel's App settings view: the inspected page's own dev
  // settings, as its SDK registered them (web/sdk/src/devtools.ts), each with
  // its effective value and where it comes from (Clax config, the site's
  // default, or you), and a reset for the viewer's own override. Native
  // controls only: a radio group in a fieldset (one Tab stop, arrows choose),
  // a checkbox with the switch role, a text field that commits on Enter or
  // when left. Everything shown is the page's text, never markup. What a
  // change did is said in the host's notices zone (`say`).
  import type { DevField, DevSection, SetResult, Source, Value } from "../../../sdk/src/devtools";

  type Link = {
    status: "loading" | "none" | "ready";
    snapshot: { origin: string; sections: DevSection[] } | null;
    set(section: string, field: string, value: Value): Promise<SetResult>;
    reset(section: string, field: string): Promise<SetResult>;
  };
  let { link, say }: { link: Link; say(text: string, reload?: boolean): void } = $props();

  const SOURCE: Record<Source, string> = { clax: "Clax config", site: "Site default", you: "You" };
  const shown = (f: Pick<DevField, "kind" | "options">, v: Value) =>
    typeof v === "boolean" ? (v ? "On" : "Off") : f.kind === "radio" ? (f.options?.find(o => o.value === v)?.label ?? v) : v === "" ? "empty" : `“${v}”`;
  const fid = (s: DevSection, f: DevField) => `as-${s.id}-${f.id}`;
  /** Text fields being edited, by id: what the person typed, not yet committed. */
  let drafts = $state<Record<string, string>>({});

  async function done(f: DevField, r: SetResult): Promise<void> {
    if (!r.ok) { say(`${f.label} not changed: ${r.reason}`); return; }
    const reload = r.field.applies === "reload";
    say(`${f.label}: ${shown(r.field, r.field.value)} (${SOURCE[r.field.source]}).${reload ? " Reload the page to apply." : ""}`, reload);
  }
  const set = async (s: DevSection, f: DevField, v: Value) => done(f, await link.set(s.id, f.id, v));
  const reset = async (s: DevSection, f: DevField) => done(f, await link.reset(s.id, f.id));
  function commit(s: DevSection, f: DevField): void {
    const id = fid(s, f);
    const v = drafts[id];
    if (v === undefined) return;
    const { [id]: _, ...rest } = drafts;
    drafts = rest;
    if (v !== f.value) void set(s, f, v);
  }
  /** An option's description, then what it needs (Changeling's "Needs from the Clax SDK"). */
  const optionNote = (o: { description?: string; needs?: string }) =>
    [o.description, o.needs ? `Needs from the Clax SDK: ${o.needs}` : ""].filter(Boolean).join(". ");
  const resetName =(f: DevField) => f.inherited ? `Reset ${f.label} to ${SOURCE[f.inherited.source].toLowerCase()} (${shown(f, f.inherited.value)})` : `Reset ${f.label}`;
</script>

<div class="settings">
  {#if link.status === "loading"}
    <p class="hint">Reading the page’s settings…</p>
  {:else if link.status === "none" || !link.snapshot}
    <p class="hint">This page registers no app dev settings.</p>
    <p class="hint small">A page shows its own here with <code>registerDevSettings</code> from <code>@empathic/clax-sdk</code>, on a dev or local origin.</p>
  {:else}
    {@const snap = link.snapshot}
    <p class="origin">Settings of <span class="mono">{snap.origin}</span>. Your changes are kept in this browser only.</p>
    {#if !snap.sections.length}<p class="hint">This page registers no app dev settings.</p>{/if}
    {#each snap.sections as s (s.id)}
      <section class="sect" aria-labelledby="as-{s.id}">
        <h2 id="as-{s.id}">{s.title}</h2>
        {#if s.description}<p class="desc">{s.description}</p>{/if}
        {#each s.fields as f (f.id)}
          {@const id = fid(s, f)}
          <div class="field">
            {#if f.kind === "radio"}
              <fieldset aria-describedby="{id}-src">
                <legend>{f.label}</legend>
                {#each f.options ?? [] as o, i (o.value)}
                  {@const note = optionNote(o)}
                  <div class="opt" class:off={o.disabled}>
                    <input type="radio" id="{id}-{i}" name={id} value={o.value} checked={f.value === o.value} disabled={o.disabled || f.locked}
                      aria-describedby={note ? `${id}-${i}-d` : undefined}
                      onchange={() => set(s, f, o.value)} />
                    <label for="{id}-{i}">{o.label}</label>
                    {#if note}<small id="{id}-{i}-d">{note}</small>{/if}
                  </div>
                {/each}
              </fieldset>
            {:else if f.kind === "toggle"}
              <label class="toggle">
                <input type="checkbox" role="switch" checked={f.value === true} disabled={f.locked} aria-describedby="{id}-src"
                  onchange={e => set(s, f, e.currentTarget.checked)} />
                <span>{f.label}</span>
              </label>
            {:else}
              <label class="text" for={id}>{f.label}</label>
              <input {id} type="text" maxlength={f.maxLength} disabled={f.locked} aria-describedby="{id}-src"
                value={drafts[id] ?? String(f.value)}
                oninput={e => (drafts = { ...drafts, [id]: e.currentTarget.value })}
                onkeydown={e => { if (e.key === "Enter") commit(s, f); }}
                onblur={() => commit(s, f)} />
            {/if}
            {#if f.description}<p class="desc">{f.description}</p>{/if}
            <p class="src" id="{id}-src">
              From: {SOURCE[f.source]}{#if f.inherited}, over {SOURCE[f.inherited.source].toLowerCase()} {shown(f, f.inherited.value)}{/if}.
              {#if f.applies === "reload"}Applies on reload.{/if}
              {#if f.locked}Set by the Clax config; not changeable here.{/if}
            </p>
            {#if f.source === "you"}
              <button type="button" class="ghost" onclick={() => reset(s, f)}>{resetName(f)}</button>
            {/if}
          </div>
        {/each}
      </section>
    {/each}
  {/if}
</div>

<style>
  .settings { padding: 8px var(--gutter) 18px; }
  .hint { color: var(--muted); margin: 12px 0 0; }
  .small { font-size: 12.5px; }
  .origin { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); }
  .mono, code { font: 12px var(--mono); }
  .sect { margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--border); }
  h2 { margin: 0; font-size: 14px; font-weight: 600; }
  .desc { margin: 2px 0 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .field { margin-top: 12px; display: grid; gap: 4px; justify-items: start; }
  fieldset { margin: 0; padding: 0; border: 0; min-width: 0; display: grid; gap: 2px; }
  legend { padding: 0; font-weight: 500; margin-bottom: 4px; }
  .opt { display: grid; grid-template-columns: auto 1fr; column-gap: 8px; align-items: baseline; min-height: 24px; padding: 2px 0; }
  .opt small { grid-column: 2; color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }
  .opt.off label { color: var(--muted); }
  .toggle { display: flex; gap: 8px; align-items: center; min-height: 24px; font-weight: 500; }
  .text { font-weight: 500; }
  input[type="text"] { width: 100%; max-width: 32rem; box-sizing: border-box; }
  .src { margin: 0; font-size: 12px; color: var(--muted); }
  button.ghost { font-size: 12.5px; min-height: 24px; }
  @media (forced-colors: active) { .sect { border-color: CanvasText; } }
</style>
