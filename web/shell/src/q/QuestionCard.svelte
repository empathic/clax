<svelte:options css="injected" />

<script lang="ts">
  // One agent question (spec 2026-10-06-agent-questions-and-inbox-design
  // §9.1): who asks and about what, one chip per question (tabs when
  // several, each marked answered with its answer under its header, or
  // marked not answered), options with descriptions and
  // a Recommended chip (a mirrored label's trailing "(Recommended)" is not
  // shown twice), "Other…", a preview beside the options when the card is
  // wide (stacked when narrow), free text, and Answer / Skip / Answer in
  // the terminal. With several questions, picking a single choice's option
  // (a click, Space or a digit) moves on to the next unanswered question
  // after a moment; a multi choice and "Other" text wait for the person to
  // move. Back and Next move one question either way, and the primary
  // action says how many are left ("Answer 2 more"), pressed early, it goes
  // to the first unanswered question. Keys while focus is in the card:
  // arrows between options, Space toggles, 1–4 pick, B and N back and
  // next, Enter answers once every question has an answer (before that it
  // goes to the first unanswered). Answer, Skip and Answer in the terminal follow the keyboard
  // trail rule (`guardedAction`). A closed question says what closed it and
  // shows its answers; a card that closes while mounted announces it (and,
  // when something else closed it first, that the person's action was not
  // taken), and keeps focus when it held it. Every string from the agent is
  // text; a question's text is Markdown (`ui/Markdown.svelte`), and previews
  // are verbatim in a <pre>. Its styles travel with the question module's lazy
  // chunk (injected on mount). State shows in data attributes, not class
  // directives, and the card finds its element through its events: the
  // runtime helpers for those belong to the artifact entry's chunk. The text
  // fields hear `input` in the capture phase, as no delegated handler would
  // hear one that does not bubble; a binding's helper would be split out of
  // the sidebar's chunk.
  import { tick, untrack } from "svelte";
  import { ApiError, type AnswerBody, type QuestionStatus, type QuestionView } from "../api";
  import { after } from "../clock";
  import { relativeTime } from "../format";
  import { aboutLink, agentLabel, answerLine, answerText, closedLabel, complete, cutHeader, emptyDraft, nextOpen, pick, previewOf, questionLink, shownLabel, toBody, typeOther } from "./model";
  import { trail } from "./trail";
  import Markdown from "../ui/Markdown.svelte";
  import RetryButton from "../ui/RetryButton.svelte";

  type Props = {
    q: QuestionView;
    /** The questions listed beside it: an agent is named by harness alone unless another of its harness is among them. */
    others?: QuestionView[];
    /** The artifact shown around the card: its link is left out. */
    here?: string | null;
    /** The thread whose card holds this one: its link is left out. */
    thread?: string | null;
    now?: Date;
    onAnswer(b: AnswerBody): Promise<void>;
    onDecline(): Promise<void>;
    /** "Answer in the terminal" (mirrored questions only). */
    onRelease?(): Promise<void>;
  };
  /** How long a picked single choice shows before the card moves on. */
  const ADVANCE_MS = 250;
  const p: Props = $props();
  let draft = $state(untrack(() => emptyDraft(p.q)));
  let tab = $state(0);
  let focused = $state<string | null>(null);
  let busy = $state(false);
  let hint = $state<string | null>(null);
  // The card's element, from its own focus and key events.
  let card: HTMLElement | undefined;
  // Focus is in the card. Focus that leaves to nowhere because its control
  // was disabled (a pending action) or removed (the card closing) still
  // counts, so the card can take it back instead of leaving it on <body>.
  let inside = false;
  // What a close says to assistive technology; empty on a card mounted closed.
  let said = $state("");
  // Shown under the closed label when the person's action was not taken ("Not answered").
  let notTaken = $state("");
  // The status the person's pending action would close the question with.
  let pending: QuestionStatus | null = null;
  // Whether the card was open when this instance last looked.
  let wasOpen = untrack(() => p.q.status === "open");
  $effect(() => trail().keyboardTrail.onClear(() => { hint = null; }));
  const who = $derived(agentLabel(p.q.agent, (p.others ?? [p.q]).map(o => o.agent)));
  // The Clax app's agent: its card names Clax, never its project folder, and its button reads "Answer".
  const app = $derived(p.q.agent?.harness === "app");
  const done = $derived(complete(p.q, draft));
  const ready = $derived(done.every(Boolean));
  const n = $derived(p.q.questions.length);
  const left = $derived(done.filter(d => !d).length);
  const spec =$derived(p.q.questions[Math.min(tab, p.q.questions.length - 1)]);
  const preview = $derived(previewOf(spec, focused, draft[tab]));
  const open = $derived(p.q.status === "open");
  // The view of the question a Retry of its copy succeeded on
  // (`RetryButton`): until a newer view comes, the copy reads as sending again.
  let retriedOn = $state.raw<QuestionView | null>(null);
  const link = $derived(questionLink(p.q, retriedOn === p.q));
  const about = $derived(aboutLink(p.q, p.here, p.thread));
  const base = $derived(`q-${p.q.id}`);
  // Takes focus back only when it went nowhere (<body>): focus the person
  // moved elsewhere while the action was pending stays where they put it.
  const regain = () => {
    const a = document.activeElement;
    if (inside && card?.isConnected && (!a || a === document.body)) card.focus();
  };
  const NOT_TAKEN: Partial<Record<QuestionStatus, string>> = { answered: "Not answered", declined: "Not skipped", released: "Not moved" };
  $effect(() => {
    if (open) { wasOpen = true; return; }
    stopAdvance();
    if (!wasOpen) return;
    wasOpen = false;
    const label = closedLabel(p.q);
    const was = pending;
    const instead = was !== null && (p.q.status !== was || (was === "answered" && p.q.answered_via === "terminal"));
    notTaken = instead ? NOT_TAKEN[was!]! : "";
    said = instead ? `${notTaken}: ${label}` : label;
    pending = null;
    regain();
  });
  const onFocusOut = (e: FocusEvent) => {
    const to = e.relatedTarget as Node | null;
    if (to) { inside = !!card?.contains(to); return; }
    const from = e.target as HTMLElement;
    // Settle once the change that moved focus has applied.
    queueMicrotask(() => { inside = !from.isConnected || (from as HTMLButtonElement).disabled === true; });
  };

  /** The hint for a failed action: the daemon's message for a refusal, else the connection. */
  const failed = (err: unknown): string =>
    err instanceof ApiError && err.status ? `Not sent: ${err.message.replace(/^\d+ /, "")}` : "Not sent: Clax did not answer. Try again.";
  // Runs `f` for activation `e` of a consequential action unless the trail says to click.
  const run = (e: Event, verb: string, closes: QuestionStatus, f: () => Promise<void>) => {
    if (busy) return;
    hint = trail().guardedAction(e, verb, () => {
      busy = true;
      pending = closes;
      f().then(() => { busy = false; }, err => { busy = false; pending = null; hint = failed(err); queueMicrotask(regain); });
    });
  };
  // The pending move on after a pick, if any.
  let advancing: (() => void) | undefined;
  const stopAdvance = () => { advancing?.(); advancing = undefined; };
  $effect(() => stopAdvance);
  const show = (i: number) => { stopAdvance(); tab = i; focused = null; };
  // Focuses the shown question's picked option, else its first control, once it is rendered.
  const focusPart = () => tick().then(() => {
    const body = card?.querySelector(".panel .body");
    (body?.querySelector<HTMLElement>("input[data-opt]:checked") ?? body?.querySelector<HTMLElement>("input, textarea"))?.focus();
  });
  // Shows question `i`; `focus` moves focus to it, for a move that removes the focused control.
  const go = (i: number, focus: boolean) => {
    if (i < 0 || i >= n) return;
    show(i);
    if (focus) void focusPart();
  };
  // The card for an event from inside it.
  const from = (e: Event) => { card = (e.currentTarget as HTMLElement).closest<HTMLElement>(".qcard") ?? card; };
  const answer = (e: Event) => {
    from(e);
    if (ready) run(e, "answer", "answered", () => p.onAnswer(toBody(p.q, draft)));
    else if (n > 1) go(nextOpen(done, -1), true);
  };
  // Picks `label` on the shown question. A single choice `advance`s: after
  // ADVANCE_MS the card shows the next unanswered question, if any, taking
  // focus there when it is in the card.
  const choose = (label: string, advance = false) => {
    draft = pick(p.q, tab, label, draft);
    stopAdvance();
    if (!advance || n < 2 || spec.multi_select) return;
    const at = tab;
    advancing = after(ADVANCE_MS, () => {
      advancing = undefined;
      const i = nextOpen(complete(p.q, draft), at);
      if (!open || tab !== at || i < 0) return;
      go(i, inside);
    });
  };
  const other = { get: () => draft[tab].text, set: (v: string) => { stopAdvance(); draft = typeOther(p.q, tab, v, draft); } };

  const optionInputs = () => [...(card?.querySelectorAll<HTMLInputElement>(".opt input[data-opt]") ?? [])];
  const onKey = (e: KeyboardEvent) => {
    card = e.currentTarget as HTMLElement;
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
    const onTab = t.getAttribute("role") === "tab";
    const focusTab = () => queueMicrotask(() => card?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus());
    if (onTab && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      show((tab + (e.key === "ArrowRight" ? 1 : n - 1)) % n);
      focusTab();
      return;
    }
    const move = n > 1 ? ({ b: -1, n: 1 } as Record<string, number>)[e.key.toLowerCase()] : undefined;
    if (move) {
      e.preventDefault();
      // Focus on an option would go with its question; focus on a tab follows the selection.
      go(tab + move, !!t.closest(".body"));
      if (onTab) focusTab();
      return;
    }
    const inputs = optionInputs();
    if (/^[1-4]$/.test(e.key)) {
      const o = spec.options[Number(e.key) - 1];
      if (!o) return;
      e.preventDefault();
      choose(o.label, true);
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

</script>

<!-- Keys act only while focus is in the card; every control is reachable by Tab, so the article's handler adds shortcuts, not a path. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
<article class="qcard" data-closed={open ? undefined : ""} data-question={p.q.id} aria-label={`${who} asks`} tabindex="-1"
  onkeydown={onKey} onfocusin={e => { card = e.currentTarget; inside = true; }} onfocusout={onFocusOut}>
  <header class="qhead">
    <strong class="asker">{who}</strong>
    {#if link}<span class="link-tag" data-unlinked={p.q.linked?.unlinked ? "" : undefined} title={link.title}>{link.label}</span>{/if}
    {#if p.q.agent?.project && !app}<span class="proj">in {p.q.agent.project}</span>{/if}
    {#if about}<a class="about" href={about.href}>{about.text}</a>{/if}
    <time datetime={p.q.created_at}>{relativeTime(p.q.created_at, p.now ?? new Date())}</time>
  </header>
  {#if !open}
    <p class="closed">{closedLabel(p.q)}{#if link?.line}<span class="there">{` ${link.line}`}</span>{/if}</p>
    <!-- The status region says the whole sentence; this is its visible part. -->
    {#if notTaken}<p class="instead" aria-hidden="true">{notTaken}</p>{/if}
    <dl class="answers">
      {#each p.q.questions as s, i (s.question)}
        <div><dt>{cutHeader(s.header)}</dt><dd class="q"><Markdown text={s.question} /></dd>{#if p.q.answers}<dd class="a">{answerText(p.q, i)}</dd>{/if}</div>
      {/each}
    </dl>
  {:else}
    {#if p.q.questions.length > 1}
      <div class="chips" role="tablist" aria-label="Questions">
        {#each p.q.questions as s, i (s.question)}
          {@const line = done[i] ? answerLine(s, { selected: draft[i].selected, text: draft[i].text.trim() }) : ""}
          <button type="button" class="chip" data-done={done[i] ? "" : undefined} role="tab" id={`${base}-tab-${i}`} aria-controls={`${base}-panel`} aria-selected={tab === i}
            title={s.header} aria-label={done[i] ? `${s.header}, answered: ${line}` : `${s.header}, not answered`} tabindex={tab === i ? 0 : -1} onclick={() => show(i)}>
            <span class="hdr"><span class="mark" aria-hidden="true">{done[i] ? "✓" : ""}</span>{cutHeader(s.header)}</span>
            <span class="ans">{done[i] ? line : "Not answered"}</span>
          </button>
        {/each}
      </div>
    {:else}
      <span class="chip solo" title={spec.header}>{cutHeader(spec.header)}</span>
    {/if}
    <div class="panel" id={`${base}-panel`} role={p.q.questions.length > 1 ? "tabpanel" : undefined} aria-labelledby={p.q.questions.length > 1 ? `${base}-tab-${tab}` : undefined}>
      <div class="qtext" id={`${base}-text-${tab}`}><Markdown text={spec.question} /></div>
      <!-- One body per tab: a new tab's options are new inputs. -->
      {#each [tab] as t (t)}
      <div class="body" data-preview={preview !== null ? "" : undefined}>
        {#if spec.options.length}
          <div class="opts" role={spec.multi_select ? "group" : "radiogroup"} aria-labelledby={`${base}-text-${tab}`} onmouseleave={() => { focused = null; }}
            onfocusout={e => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) focused = null; }}>
            {#each spec.options as o, j (o.label)}
              <label class="opt" data-on={draft[tab].selected.includes(o.label) ? "" : undefined} onmouseenter={() => { focused = o.label; }} onfocusin={() => { focused = o.label; }}>
                <input type={spec.multi_select ? "checkbox" : "radio"} name={`${base}-${tab}`} value={o.label} data-opt
                  checked={draft[tab].selected.includes(o.label)} onclick={() => choose(o.label, true)} />
                <span class="num" aria-hidden="true">{j + 1}</span>
                <span class="txt"><span class="lbl">{shownLabel(o)}</span>{#if o.recommended}<span class="rec">Recommended</span>{/if}
                  {#if o.description}<span class="desc">{o.description}</span>{/if}</span>
              </label>
            {/each}
            {#if spec.other}
              <label class="opt other" data-on={draft[tab].text.trim() !== "" ? "" : undefined}>
                <span class="lbl">Other…</span>
                <input type="text" placeholder="Type your own answer" value={other.get()} oninputcapture={e => other.set(e.currentTarget.value)} />
              </label>
            {/if}
          </div>
          {#if preview}<pre class="preview" aria-label="Preview">{preview}</pre>
          {:else if preview === ""}<p class="preview none">No preview for this option</p>{/if}
        {:else}
          <textarea rows="3" aria-labelledby={`${base}-text-${tab}`} placeholder="Your answer" value={other.get()} oninputcapture={e => other.set(e.currentTarget.value)}></textarea>
        {/if}
      </div>
      {/each}
    </div>
    {#if link?.line}<p class="link-line" data-failed={link.failed ? "" : undefined}><span>{link.line}</span>{#if link.retry && p.q.linked && p.q.artifact}<RetryButton view={p.q} artifact={p.q.artifact.id} remote={p.q.linked.remote_id} onRetried={v => { retriedOn = v; }} />{/if}</p>{/if}
    <footer class="acts">
      {#if n > 1}
        <!-- aria-disabled, not disabled: a button disabled under focus drops it. -->
        <div class="steps" role="group" aria-label="Move between questions">
          <button type="button" aria-disabled={tab === 0} aria-keyshortcuts="B" title="Previous question (B)" onclick={() => go(tab - 1, false)}>Back</button>
          <span class="pos">{tab + 1} of {n}</span>
          <button type="button" aria-disabled={tab === n - 1} aria-keyshortcuts="N" title="Next question (N)" onclick={() => go(tab + 1, false)}>Next</button>
        </div>
      {/if}
      <button type="button" class="primary" disabled={busy || (!ready && n < 2)} data-left={ready ? undefined : ""}
        title={ready || n < 2 ? undefined : "Go to the first unanswered question"} onclick={answer}>{ready || n < 2 ? (app ? "Answer" : `Answer ${who}`) : `Answer ${left} more`}</button>
      <button type="button" disabled={busy} onclick={e => run(e, "skip", "declined", p.onDecline)}>Skip</button>
      {#if p.q.source === "hook" && p.onRelease}
        <button type="button" disabled={busy} onclick={e => run(e, "move it to the terminal", "released", p.onRelease!)}>Answer in the terminal</button>
      {/if}
    </footer>
  {/if}
  <!-- One live region for the card's life: while open, the trail's hint (a key
       asked for an action on a trail the page may have steered) or a failure;
       once closed, what closed it (read, not shown: the card shows it). -->
  <p class="act-hint" data-sr={open ? undefined : ""} role="status">{open ? (hint ?? "") : said}</p>
</article>

<style>
  :global {
    .qcard { container-type: inline-size; position: relative; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--you); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; }
    .qcard:focus { outline: none; }
    .qcard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .qcard[data-closed] { border-left-color: var(--border-strong); }
    .qcard .qhead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; font-size: 12.5px; color: var(--muted); margin-bottom: 8px; min-width: 0; }
    .qcard .asker { font: 600 13px/20px var(--font); color: var(--agent-ink); }
    .qcard .about { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-hover); text-underline-offset: 2px; }
    .qcard .about:hover { text-decoration-color: currentColor; }
    .qcard time { margin-left: auto; white-space: nowrap; }
    .qcard .chips { display: grid; grid-template-columns: repeat(auto-fill, minmax(104px, 1fr)); gap: 6px; margin-bottom: 10px; }
    .qcard .chips .chip { display: grid; justify-items: start; align-content: start; gap: 2px; min-width: 0; min-height: 0; padding: 5px 8px 6px; text-align: left; font: 500 12px/16px var(--font); border-radius: var(--radius-sm); border: 1px dashed var(--border-strong); background: var(--card); color: var(--muted); }
    .qcard .chips .chip[data-done] { border-style: solid; border-color: var(--border-hover); }
    .qcard .chips .chip[aria-selected="true"] { border: 1px solid var(--fg); color: var(--fg); background: var(--hover); box-shadow: inset 0 -2px 0 var(--fg); }
    .qcard .chip .hdr { display: flex; align-items: center; gap: 5px; max-width: 100%; overflow: hidden; white-space: nowrap; }
    .qcard .chip .mark { flex: none; display: inline-grid; place-items: center; width: 12px; height: 12px; border-radius: 50%; border: 1.5px solid var(--border-strong); font: 700 9px/1 var(--font); }
    .qcard .chip[data-done] .mark { border-color: var(--agent); background: var(--agent); color: var(--on-accent); }
    .qcard .chip .ans { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 400; color: var(--fg); }
    .qcard .chip:not([data-done]) .ans { color: var(--muted); font-style: italic; }
    .qcard .chip.solo { display: inline-flex; align-items: center; margin-bottom: 6px; font: 500 12px/1 var(--font); padding: 4px 10px; border-radius: 999px; color: var(--fg); background: var(--hover); }
    .qcard .qtext { margin: 0 0 8px; font: 600 14px/1.45 var(--font); white-space: pre-wrap; overflow-wrap: anywhere; }
    .qcard .body { min-width: 0; }
    .qcard .body[data-preview] { display: grid; gap: 10px; }
    @container (min-width: 560px) { .qcard .body[data-preview] { grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr); align-items: start; } }
    .qcard .opts { display: grid; gap: 6px; min-width: 0; }
    .qcard .opt { display: grid; grid-template-columns: auto auto minmax(0, 1fr); align-items: start; gap: 2px 8px; padding: 7px 9px; border: 1px solid var(--border); border-radius: var(--radius-sm); cursor: pointer; transition: border-color var(--t), background-color var(--t); }
    .qcard .opt:hover { border-color: var(--border-hover); background: var(--hover); }
    .qcard .opt[data-on] { border-color: var(--border-strong); background: var(--hover); }
    .qcard .opt input[data-opt] { margin: 3px 0 0; width: 16px; height: 16px; min-height: 0; accent-color: var(--fg); }
    .qcard .num { font: 500 10.5px/14px var(--mono); color: var(--muted); border: 1px solid var(--border-hover); border-radius: var(--radius-xs); padding: 0 4px; margin-top: 2px; }
    .qcard .txt { display: grid; gap: 2px; min-width: 0; }
    .qcard .lbl { font-weight: 500; overflow-wrap: anywhere; }
    .qcard .rec { justify-self: start; font: 500 11px/16px var(--font); padding: 0 6px; border-radius: 999px; background: var(--accent-tint); color: var(--agent-ink); }
    .qcard .desc { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .qcard .opt.other { grid-template-columns: auto minmax(0, 1fr); align-items: center; cursor: text; }
    .qcard .opt.other .lbl { color: var(--muted); font-weight: 500; }
    .qcard .opt.other input { width: 100%; min-width: 0; }
    .qcard .preview { margin: 0; max-height: 320px; overflow: auto; white-space: pre; font: 12px/1.45 var(--mono); padding: 10px 12px; background: var(--bg); color: var(--fg); border: 1px solid var(--border); border-radius: var(--radius-sm); tab-size: 4; }
    .qcard .preview.none { white-space: normal; font: 12.5px var(--font); color: var(--muted); }
    .qcard textarea { display: block; width: 100%; resize: vertical; min-height: 64px; }
    .qcard .acts { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; margin-top: 10px; }
    .qcard .acts .primary { max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
    .qcard .acts .primary[data-left] { background: var(--card); color: var(--fg); border-color: var(--border-strong); }
    .qcard .acts .primary[data-left]:hover { background: var(--hover); border-color: var(--border-strong); }
    .qcard .steps { display: flex; align-items: center; gap: 6px; margin-right: auto; }
    .qcard .steps .pos { font: 500 12px var(--mono); color: var(--muted); white-space: nowrap; }
    .qcard .steps button[aria-disabled="true"] { opacity: .5; cursor: default; }
    .qcard .closed { margin: 0 0 6px; font: 600 13px/20px var(--font); color: var(--fg); }
    .qcard .answers { margin: 0; display: grid; gap: 6px; }
    .qcard .answers > div { display: grid; gap: 1px; min-width: 0; }
    .qcard .answers dt { font: 500 11.5px var(--font); color: var(--muted); }
    .qcard .answers dd { margin: 0; overflow-wrap: anywhere; white-space: pre-wrap; }
    .qcard .answers .q { font-size: 12.5px; color: var(--muted); }
    .qcard .answers .a { font-size: 13.5px; }
    .qcard .link-tag { display: inline-flex; align-self: center; flex: none; padding: 0 7px; border-radius: 999px; font: 600 11px/17px var(--font); color: var(--muted); background: var(--hover); border: 1px solid var(--border); white-space: nowrap; max-width: 40%; overflow: hidden; text-overflow: ellipsis; }
    .qcard .link-tag[data-unlinked] { font-weight: 500; font-style: italic; }
    .qcard .link-line { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); }
    .qcard .link-line[data-failed] { color: var(--danger); }
    .qcard .link-line[data-failed] { display: flex; align-items: center; gap: 8px; }
    .qcard .link-line .retry { margin-left: auto; flex: none; min-height: 24px; padding: 2px 10px; font-size: 12px; color: var(--fg); }
    .qcard .closed .there { font-weight: 400; color: var(--muted); }
    .qcard .instead { margin: -2px 0 6px; font-size: 12.5px; color: var(--danger); }
    @media (pointer: coarse) { .qcard .opt input[data-opt] { width: 20px; height: 20px; } }
  }
</style>
