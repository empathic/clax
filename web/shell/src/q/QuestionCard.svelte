<svelte:options css="injected" />

<script lang="ts">
  // One agent question (spec 2026-10-06-agent-questions-and-inbox-design
  // §9.1): who asks and about what, one chip per question (tabs when
  // several, each marked done when answered), options with descriptions and
  // a Recommended chip (left out when a mirrored label already ends with
  // "(Recommended)"), "Other…", a preview beside the options when the
  // card is wide (stacked when narrow), free text, and Answer / Skip /
  // Answer in the terminal. Keys while focus is in the card: arrows between
  // options, Space toggles, 1–4 pick, Enter answers once every question has
  // an answer. Answer, Skip and Answer in the terminal follow the keyboard
  // trail rule (`guardedAction`). A closed question says what closed it and
  // shows its answers. Every string from the agent is text; previews are
  // verbatim in a <pre>. Its styles travel with the question module's lazy
  // chunk (injected on mount).
  import { untrack } from "svelte";
  import type { AnswerBody, QuestionView } from "../api";
  import { relativeTime } from "../format";
  import { guardedAction, keyboardTrail } from "../view/trail";
  import { agentLabel, closedLabel, complete, cutHeader, emptyDraft, pick, previewOf, toBody, typeOther } from "./model";

  type Props = {
    q: QuestionView;
    /** The questions listed beside it: an agent is named by harness alone unless another of its harness is among them. */
    others?: QuestionView[];
    /** The artifact shown around the card: its link is left out. */
    here?: string | null;
    now?: Date;
    onAnswer(b: AnswerBody): Promise<void>;
    onDecline(): Promise<void>;
    /** "Answer in the terminal" (mirrored questions only). */
    onRelease?(): Promise<void>;
  };
  const p: Props = $props();
  let draft = $state(untrack(() => emptyDraft(p.q)));
  let tab = $state(0);
  let focused = $state<string | null>(null);
  let busy = $state(false);
  let hint = $state<string | null>(null);
  let card: HTMLElement | undefined = $state();
  // Focus was in the card when it last moved: a card that closes under it takes focus itself, not <body>.
  let inside = false;
  $effect(() => keyboardTrail.onClear(() => { hint = null; }));
  const who = $derived(agentLabel(p.q.agent, (p.others ?? [p.q]).map(o => o.agent)));
  const done = $derived(complete(p.q, draft));
  const ready = $derived(done.every(Boolean));
  const spec = $derived(p.q.questions[Math.min(tab, p.q.questions.length - 1)]);
  const preview = $derived(previewOf(spec, focused, draft[tab]));
  const open = $derived(p.q.status === "open");
  const base = $derived(`q-${p.q.id}`);
  $effect(() => { if (!open && inside) card?.focus(); });

  // Runs `f` for activation `e` of a consequential action unless the trail says to click.
  const run = (e: Event, verb: string, f: () => Promise<void>) => {
    if (busy) return;
    hint = guardedAction(e, verb, () => {
      busy = true;
      f().then(() => { busy = false; }, () => { busy = false; hint = "Not sent: Clax did not answer. Try again."; });
    });
  };
  const answer = (e: Event) => { if (ready) run(e, "answer", () => p.onAnswer(toBody(p.q, draft))); };
  const choose = (label: string) => { draft = pick(p.q, tab, label, draft); };
  const other = { get: () => draft[tab].text, set: (v: string) => { draft = typeOther(p.q, tab, v, draft); } };
  const show = (i: number) => { tab = i; focused = null; };

  const optionInputs = () => [...(card?.querySelectorAll<HTMLInputElement>(".opt input[data-opt]") ?? [])];
  const onKey = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
    const t = e.target as HTMLElement;
    const typing = t instanceof HTMLTextAreaElement || (t instanceof HTMLInputElement && t.type === "text");
    if (e.key === "Enter") {
      if (t instanceof HTMLButtonElement || t instanceof HTMLAnchorElement || (typing && e.shiftKey)) return;
      e.preventDefault();
      answer(e);
      return;
    }
    if (typing) return;
    if (t.getAttribute("role") === "tab" && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      const n = p.q.questions.length;
      show((tab + (e.key === "ArrowRight" ? 1 : n - 1)) % n);
      queueMicrotask(() => card?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus());
      return;
    }
    const inputs = optionInputs();
    if (/^[1-4]$/.test(e.key)) {
      const o = spec.options[Number(e.key) - 1];
      if (!o) return;
      e.preventDefault();
      choose(o.label);
      inputs[Number(e.key) - 1]?.focus();
      return;
    }
    const at = inputs.indexOf(t as HTMLInputElement);
    const step = e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : e.key === "ArrowUp" || e.key === "ArrowLeft" ? -1 : 0;
    if (at < 0 || !step) return;
    e.preventDefault();
    const next = (at + step + inputs.length) % inputs.length;
    inputs[next].focus();
    // As a radio group does: moving in a single choice picks.
    if (!spec.multi_select) choose(spec.options[next].label);
  };

  const answerText = (i: number): string => {
    const a = p.q.answers?.[i];
    if (!a) return "—";
    const parts = [...a.selected, ...(a.text ? [p.q.questions[i].options.length ? `Other: ${a.text}` : a.text] : [])];
    return parts.length ? parts.join(", ") : "—";
  };
</script>

<!-- Keys act only while focus is in the card; every control is reachable by Tab, so the article's handler adds shortcuts, not a path. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
<article class="qcard" class:closed-card={!open} data-question={p.q.id} aria-label={`${who} asks`} tabindex="-1" bind:this={card}
  onkeydown={onKey} onfocusin={() => { inside = true; }} onfocusout={e => { inside = !!card?.contains(e.relatedTarget as Node | null); }}>
  <header class="qhead">
    <strong class="who">{who}</strong>
    {#if p.q.agent?.project}<span class="proj">in {p.q.agent.project}</span>{/if}
    {#if p.q.artifact && p.q.artifact.id !== p.here}<a class="about" href={`/a/${p.q.artifact.id}`}>{p.q.artifact.title}</a>{/if}
    <time datetime={p.q.created_at}>{relativeTime(p.q.created_at, p.now ?? new Date())}</time>
  </header>
  {#if !open}
    <p class="closed">{closedLabel(p.q)}</p>
    <dl class="answers">
      {#each p.q.questions as s, i (s.question)}
        <div><dt>{cutHeader(s.header)}</dt><dd class="q">{s.question}</dd>{#if p.q.answers}<dd class="a">{answerText(i)}</dd>{/if}</div>
      {/each}
    </dl>
  {:else}
    {#if p.q.questions.length > 1}
      <div class="chips" role="tablist" aria-label="Questions">
        {#each p.q.questions as s, i (s.question)}
          <button type="button" class="chip" class:done={done[i]} role="tab" id={`${base}-tab-${i}`} aria-controls={`${base}-panel`} aria-selected={tab === i}
            tabindex={tab === i ? 0 : -1} onclick={() => show(i)}>{#if done[i]}<span class="tick" aria-hidden="true">✓</span>{/if}{cutHeader(s.header)}</button>
        {/each}
      </div>
    {:else}
      <span class="chip solo">{cutHeader(spec.header)}</span>
    {/if}
    <div class="panel" id={`${base}-panel`} role={p.q.questions.length > 1 ? "tabpanel" : undefined} aria-labelledby={p.q.questions.length > 1 ? `${base}-tab-${tab}` : undefined}>
      <p class="qtext" id={`${base}-text-${tab}`}>{spec.question}</p>
      {#key tab}
      <div class="body" class:with-preview={preview !== null}>
        {#if spec.options.length}
          <div class="opts" role={spec.multi_select ? "group" : "radiogroup"} aria-labelledby={`${base}-text-${tab}`} onmouseleave={() => { focused = null; }}>
            {#each spec.options as o, j (o.label)}
              <label class="opt" class:on={draft[tab].selected.includes(o.label)} onmouseenter={() => { focused = o.label; }} onfocusin={() => { focused = o.label; }}>
                <input type={spec.multi_select ? "checkbox" : "radio"} name={`${base}-${tab}`} value={o.label} data-opt
                  checked={draft[tab].selected.includes(o.label)} onclick={() => choose(o.label)} />
                <span class="num" aria-hidden="true">{j + 1}</span>
                <span class="txt"><span class="lbl">{o.label}</span>{#if o.recommended && !/\(recommended\)\s*$/i.test(o.label)}<span class="rec">Recommended</span>{/if}
                  {#if o.description}<span class="desc">{o.description}</span>{/if}</span>
              </label>
            {/each}
            {#if spec.other}
              <label class="opt other" class:on={draft[tab].text.trim() !== ""}>
                <span class="lbl">Other…</span>
                <input type="text" placeholder="Type your own answer" bind:value={other.get, other.set} />
              </label>
            {/if}
          </div>
          {#if preview}<pre class="preview" aria-label="Preview">{preview}</pre>
          {:else if preview === ""}<p class="preview none">No preview for this option</p>{/if}
        {:else}
          <textarea rows="3" aria-labelledby={`${base}-text-${tab}`} placeholder="Your answer" bind:value={other.get, other.set}></textarea>
        {/if}
      </div>
      {/key}
    </div>
    <footer class="acts">
      <button type="button" class="primary" disabled={busy || !ready} onclick={answer}>Answer {who}</button>
      <button type="button" disabled={busy} onclick={e => run(e, "skip", p.onDecline)}>Skip</button>
      {#if p.q.source === "hook" && p.onRelease}
        <button type="button" disabled={busy} onclick={e => run(e, "move it to the terminal", p.onRelease!)}>Answer in the terminal</button>
      {/if}
    </footer>
    <!-- Said when a key asked for an action on a trail the page may have steered (`keyboardTrail`). -->
    <p class="act-hint" role="status">{hint ?? ""}</p>
  {/if}
</article>

<style>
  .qcard { container-type: inline-size; position: relative; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--you); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; }
  .qcard:focus { outline: none; }
  .qcard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
  .qcard.closed-card { border-left-color: var(--border-strong); }
  .qhead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; font-size: 12.5px; color: var(--muted); margin-bottom: 8px; min-width: 0; }
  .who { font: 600 13px/20px var(--font); color: var(--agent-ink); }
  .about { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-hover); text-underline-offset: 2px; }
  .about:hover { text-decoration-color: currentColor; }
  time { margin-left: auto; white-space: nowrap; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
  .chip { font: 500 12px/1 var(--font); min-height: 26px; padding: 4px 10px; border-radius: 999px; border: 1px solid var(--border-hover); background: var(--card); color: var(--muted); gap: 4px; }
  .chip[aria-selected="true"] { border-color: var(--fg); color: var(--fg); background: var(--hover); }
  .chip.done .tick { color: var(--agent-ink); font-weight: 700; }
  .chip.solo { display: inline-flex; align-items: center; min-height: 0; margin-bottom: 6px; color: var(--fg); background: var(--hover); border-color: transparent; }
  .qtext { margin: 0 0 8px; font: 600 14px/1.45 var(--font); white-space: pre-wrap; overflow-wrap: anywhere; }
  .body { min-width: 0; }
  .body.with-preview { display: grid; gap: 10px; }
  @container (min-width: 560px) { .body.with-preview { grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr); align-items: start; } }
  .opts { display: grid; gap: 6px; min-width: 0; }
  .opt { display: grid; grid-template-columns: auto auto minmax(0, 1fr); align-items: start; gap: 2px 8px; padding: 7px 9px; border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; transition: border-color var(--t), background-color var(--t); }
  .opt:hover { border-color: var(--border-hover); background: var(--hover); }
  .opt.on { border-color: var(--border-strong); background: var(--hover); }
  .opt input[data-opt] { margin: 3px 0 0; width: 16px; height: 16px; min-height: 0; accent-color: var(--fg); }
  .num { font: 500 10.5px/14px var(--mono); color: var(--muted); border: 1px solid var(--border-hover); border-radius: var(--radius-xs); padding: 0 4px; margin-top: 2px; }
  .txt { display: grid; gap: 2px; min-width: 0; }
  .lbl { font-weight: 500; overflow-wrap: anywhere; }
  .rec { justify-self: start; font: 500 11px/16px var(--font); padding: 0 6px; border-radius: 999px; background: var(--accent-tint); color: var(--agent-ink); }
  .txt .rec { margin-left: 0; }
  .desc { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .opt.other { grid-template-columns: auto minmax(0, 1fr); align-items: center; cursor: text; }
  .opt.other .lbl { color: var(--muted); font-weight: 500; }
  .opt.other input { width: 100%; min-width: 0; }
  .preview { margin: 0; max-height: 320px; overflow: auto; white-space: pre; font: 12px/1.45 var(--mono); padding: 10px 12px; background: var(--bg); color: var(--fg); border: 1px solid var(--border); border-radius: var(--radius-sm); tab-size: 4; }
  .preview.none { white-space: normal; font: 12.5px var(--font); color: var(--muted); }
  textarea { display: block; width: 100%; resize: vertical; min-height: 64px; }
  .acts { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; margin-top: 10px; }
  .acts .primary { max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
  .closed { margin: 0 0 6px; font: 600 13px/20px var(--font); color: var(--fg); }
  .answers { margin: 0; display: grid; gap: 6px; }
  .answers > div { display: grid; gap: 1px; min-width: 0; }
  .answers dt { font: 500 11.5px var(--font); color: var(--muted); }
  .answers dd { margin: 0; overflow-wrap: anywhere; white-space: pre-wrap; }
  .answers .q { font-size: 12.5px; color: var(--muted); }
  .answers .a { font-size: 13.5px; }
  .act-hint { margin: 6px 0 0; font-size: 12px; color: var(--muted); text-align: right; }
  .act-hint:empty { margin: 0; }
  @media (pointer: coarse) { .opt input[data-opt] { width: 20px; height: 20px; } }
</style>
