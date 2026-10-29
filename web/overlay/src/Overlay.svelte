<script lang="ts">
  // The overlay a deployer's page embeds (`Clax.overlay`), spec
  // 2026-10-29-feedback-loop §4: Report opens the mode bar (Element, Area,
  // Screen; E, A, S, Escape); a capture opens the composer with the image
  // and its marks (Annotator), the note (required), "Include screenshot",
  // "Include recent errors (N)" and "What's sent" (Preview); Send files the
  // report and shows "Filed". "Your reports" lists the reporter's reports,
  // polled every POLL_MS only while it is open; a report waiting on a fix
  // takes the reporter's verdict ("It's fixed", or "Still broken": a note
  // and a new capture, sent as a reopen). The badge is polled while the
  // page is visible; when it rises the launcher pulses once and "Your
  // reports" next opens on the report newly waiting.
  //
  // A capture's context and metadata are asked of the host when the capture
  // starts (`Session.begin`), and the page's recent errors read then; a send
  // waits for them and for a clip still being taken. "Your reports" opened
  // over a draft hides the composer and keeps the draft, which comes back
  // when the panel closes. The composer or the panel, the notice and the
  // launcher stack up from the bottom right corner, none over another.
  import { onDestroy, untrack } from "svelte";
  import type { Report } from "../../sdk/src/types";
  import { type FlattenDeps, type Shape, browserFlatten, flatten } from "./annotate";
  import Annotator from "./Annotator.svelte";
  import { type CaptureDeps, type Captured, type Mode, captureExact, captureFor } from "./capture";
  import { displayCaptureSupported } from "../../bridge/src/display-capture";
  import {
    BadgeWatch, type Draft, POLL_MS, Poll, type ReporterApi, type Session, type Timers, canSend, carryShapes, clipOf, clipStatus, defaultShapes,
    failureText, newDraft, pageUrl, quoteOf, realTimers, reopenOf, reportOf, summaryOf, withCapture,
  } from "./model";
  import ModeBar from "./ModeBar.svelte";
  import type { Picked, Picker } from "./picker";
  import Preview from "./Preview.svelte";
  import Reports from "./Reports.svelte";

  type Props = {
    client: ReporterApi;
    doc: Document;
    /** The overlay's host element: while comment mode is on, presses on it
     * are kept from comment mode, and a click on one of its buttons is
     * passed on to that button. It is stepped aside for an exact screenshot. */
    host: Element;
    picker: (onPick: (p: Picked) => void, onCancel: () => void, onHover?: (what: string | null) => void) => Picker;
    session: Session;
    externalId?: string;
    launcher?: "corner" | false;
    canFile?: boolean;
    /** What captures use (tests replace the renders). */
    capture?: CaptureDeps;
    flatten?: FlattenDeps;
    timers?: Timers;
    onBadge?: (n: number) => void;
    onFiled?: (r: Report) => void;
  };
  let {
    client, doc, host, picker: makePicker, session, externalId, launcher = "corner", canFile = true, capture: captureDeps = {}, flatten: flattenDeps = browserFlatten,
    timers = realTimers, onBadge, onFiled,
  }: Props = $props();

  const win = untrack(() => doc.defaultView ?? window);

  /** The mode bar, while it is open: the mode on (none yet for a reopen's note). */
  let bar = $state<{ mode: Mode | null } | null>(null);
  let picking = $state(false);
  let draft = $state<Draft | null>(null);
  let clipUrl = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let filed = $state<Report | null>(null);
  let open = $state(false);
  let reports = $state<Report[] | null>(null);
  let expanded = $state<string | null>(null);
  let loadError = $state<string | null>(null);
  let count = $state<number | null>(null);
  let pulse = $state(false);
  let reportBtn = $state<HTMLButtonElement | null>(null);
  let viewBtn = $state<HTMLButtonElement | null>(null);

  /** The report newly waiting on the reporter, which "Your reports" opens on next. */
  let waitingId: string | null = null;
  /** The pick a draft's capture came from, for the exact screenshot. */
  let pickFor: { of: Draft; mode: Mode; pick: Picked | null } | null = null;
  /** What a send of a draft waits for: its capture, context and metadata. */
  let pendingFor: { of: Draft; ready: Promise<unknown> } | null = null;
  const exactSupported = untrack(() => captureDeps.supported ?? displayCaptureSupported(win));

  /** What the keyboard has outlined, for the mode bar to announce. */
  let outlined = $state<string | null>(null);
  const picker = untrack(() => makePicker(onPick, () => closeBar(), what => { outlined = what; }));
  onDestroy(() => {
    picker.destroy();
    poll.stop();
    badge.stop();
    revokeClip();
    timers.clear(noticeTimer);
    timers.clear(pulseTimer);
  });

  // ---- The mode bar ----

  /** Starts a new report in `mode` (Element by default). A "Still broken"
   * note being written is not dropped unasked: the reporter is asked first. */
  export function report(opts: { mode?: Mode } = {}): void {
    if (!canFile) return;
    if (draft?.purpose.kind === "reopen" && (draft.body.trim() !== "" || draft.clip.state !== "none")) {
      confirmDrop = { mode: opts.mode ?? "element" };
      return;
    }
    startReport(opts.mode ?? "element");
  }

  function startReport(mode: Mode): void {
    confirmDrop = null;
    if (open) closeReports();
    draft = null;
    filed = null;
    revokeClip();
    notice = null;
    bar = { mode: null };
    choose(mode);
  }

  /** Opens "Your reports". */
  export function showReports(): void {
    if (!open) toggleReports();
  }

  function choose(mode: Mode): void {
    if (!bar) return;
    bar.mode = mode;
    if (mode === "screen") {
      setPicking(false);
      startCapture("screen", null);
      return;
    }
    picker.areasOnly(mode === "area");
    setPicking(true);
  }

  function setPicking(on: boolean): void {
    picking = on;
    picker.set(on);
  }

  /** Leaves the mode bar (a reopen's note stays), and gives focus back:
   * to the note being written, else to Report. */
  function closeBar(focus = true): void {
    bar = null;
    setPicking(false);
    if (!focus) return;
    if (draft) queueMicrotask(() => noteEl?.focus());
    else reportBtn?.focus();
  }

  // E, A and S choose a mode and Escape leaves, wherever focus is but in a
  // text field (the page's, or the reopen's note).
  $effect(() => {
    if (!bar) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      const at = e.composedPath()[0];
      if (at instanceof HTMLElement && (at.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at.tagName))) return;
      const mode = ({ e: "element", a: "area", s: "screen" } as const)[e.key.toLowerCase() as "e" | "a" | "s"];
      if (e.key === "Escape") closeBar();
      else if (mode) choose(mode);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    win.addEventListener("keydown", onKey, true);
    return () => win.removeEventListener("keydown", onKey, true);
  });

  // Comment mode takes every press and click on the page, the overlay's
  // own included: while it is on, those on the overlay stop at the window,
  // and a click on one of its buttons is given to the button as a click
  // comment mode does not take (one the page did not get from the viewer).
  $effect(() => {
    if (!picking) return;
    const kinds = ["mousedown", "mouseup", "click", "touchstart", "contextmenu", "selectstart"];
    const guard = (e: Event) => {
      const path = e.composedPath();
      // The click passed on below goes on to its button.
      if (!path.includes(host) || (e.type === "click" && !e.isTrusted)) return;
      e.stopPropagation();
      if (e.type !== "click") return;
      e.preventDefault();
      const button = path.find((n): n is HTMLButtonElement => n instanceof HTMLElement && n.tagName === "BUTTON");
      button?.click();
    };
    for (const k of kinds) win.addEventListener(k, guard, true);
    return () => { for (const k of kinds) win.removeEventListener(k, guard, true); };
  });

  // ---- Capture ----

  function onPick(p: Picked): void {
    // Comment mode ends once its hook has returned.
    picking = false;
    queueMicrotask(() => picker.set(false));
    startCapture(bar?.mode === "area" ? "area" : "element", p);
  }

  /** Captures `mode` (of pick `p`), into the reopen being written or a new draft. */
  function startCapture(mode: Mode, p: Picked | null): void {
    bar = null;
    revokeClip();
    let d: Draft;
    if (draft?.purpose.kind === "reopen") {
      d = draft;
      d.anchor = null;
      d.capture = null;
      d.clip = { state: "taking" };
      d.shapes = [];
    } else {
      draft = newDraft(mode === "screen" ? null : p!.anchor, { kind: "new" }, session.diagnosticsDefault);
      d = draft;
    }
    const of = d;
    const begun = session.begin({ mode: mode === "screen" || !p ? "screen" : p.mode, method: "dom" });
    of.diagnostics = begun.diagnostics;
    pickFor = { of, mode, pick: p };
    const context = begun.context.then(c => { if (draft === of) draft.context = draft.capture ? withCapture(c, draft.capture.info) : c; });
    const metadata = begun.metadata.then(m => { if (draft === of) draft.metadata = m; });
    const taken = captureFor(mode, p, win, captureDeps).then(c => {
      if (p) picker.done();
      if (draft === of) applyCapture(c);
    });
    pendingFor = { of, ready: Promise.all([context, metadata, taken]) };
  }

  /** Puts capture `c` in the draft. Replacing the page render with the
   * exact screenshot (`carry`) moves the reporter's marks onto it; a
   * capture that fell back to the render it replaces keeps them as they are. */
  function applyCapture(c: Captured, carry = false): void {
    if (!draft) return;
    const before = draft.capture;
    if (carry && before && c.info.method !== "display") {
      draft.capture = c;
      return;
    }
    revokeClip();
    if (carry && before) {
      const moved = carryShapes($state.snapshot(draft.shapes) as Shape[], before, c);
      draft.shapes = moved.shapes;
      draft.marksNote = moved.note;
    } else {
      draft.shapes = defaultShapes(c);
      draft.marksNote = null;
    }
    draft.capture = c;
    draft.clip = clipOf(c);
    if (draft.context) draft.context = withCapture(draft.context, c.info);
    if (draft.clip.state === "taken") clipUrl = URL.createObjectURL(draft.clip.png);
  }

  let exacting = $state(false);
  /** The browser's own screenshot of the tab, in place of the page render. */
  async function useExact(): Promise<void> {
    const d = draft;
    const from = pickFor;
    if (!d || !from || from.of !== d || exacting) return;
    exacting = true;
    try {
      // The overlay steps aside with the page's data-clax-ignore elements (capture.ts).
      const c = await captureExact(from.mode, from.pick, win, { ...captureDeps, host, fallback: d.capture ?? undefined });
      if (draft === d) applyCapture(c, true);
    } finally {
      exacting = false;
    }
  }

  function revokeClip(): void {
    if (clipUrl) URL.revokeObjectURL(clipUrl);
    clipUrl = null;
  }

  /** Drops the draft and gives focus back to the Report button. */
  function cancel(): void {
    const wasReopen = draft?.purpose.kind === "reopen";
    draft = null;
    pendingFor = null;
    pickFor = null;
    bar = null;
    setPicking(false);
    revokeClip();
    if (wasReopen) showReports();
    else reportBtn?.focus();
  }

  let noticeTimer: unknown = null;
  function say(text: string): void {
    timers.clear(noticeTimer);
    notice = text;
    noticeTimer = timers.set(() => { notice = null; }, 6000);
  }

  // ---- Sending ----

  /** The clip with its marks drawn in; none when there are no marks. */
  async function flattened(d: Draft): Promise<Blob | undefined> {
    if (!d.includeClip || d.clip.state !== "taken" || d.shapes.length === 0) return undefined;
    return flatten(d.clip.png, $state.snapshot(d.shapes) as Shape[], d.capture?.scale ?? 1, flattenDeps);
  }

  async function send(): Promise<void> {
    const d = draft;
    if (!d || !canSend(d)) return;
    d.sending = true;
    d.error = null;
    try {
      if (pendingFor?.of === d) await pendingFor.ready;
      let marked: Blob | undefined;
      try {
        marked = await flattened(d);
      } catch (e) {
        // Never the unmarked clip: its redactions would be lost.
        throw new Error(`your marks could not be drawn into the screenshot (${failureText(e)}); clear them or untick Include screenshot`, { cause: e });
      }
      const purpose = d.purpose;
      if (purpose.kind === "reopen") {
        const r = await client.reopen(purpose.report.id, reopenOf(d, marked));
        if (draft !== d) return;
        merge(r);
        draft = null;
        revokeClip();
        expanded = r.id;
        showReports();
        badge.now();
        say("Sent. The report is open again.");
        return;
      }
      const r = await client.fileReport(reportOf(d, { url: pageUrl(doc), title: doc.title }, { externalId }, marked));
      if (draft !== d) return;
      draft = null;
      revokeClip();
      merge(r, true);
      filed = r;
      onFiled?.(r);
      queueMicrotask(() => viewBtn?.focus());
    } catch (e) {
      d.error = failureText(e);
    } finally {
      d.sending = false;
    }
  }

  /** Puts `r` in the list in place of its old self, or first when new. */
  function merge(r: Report, first = false): void {
    if (!reports) return;
    reports = reports.some(x => x.id === r.id) ? reports.map(x => (x.id === r.id ? r : x)) : first ? [r, ...reports] : reports;
  }

  // ---- Your reports ----

  const poll = untrack(() => new Poll(async () => {
    const page = await client.myReports();
    if (!open) return;
    reports = page.reports;
    loadError = null;
  }, timers, POLL_MS, e => { loadError = failureText(e); }));

  function toggleReports(): void {
    if (open) { closeReports(); return; }
    open = true;
    filed = null;
    if (bar && draft?.purpose.kind !== "reopen") closeBar(false);
    if (waitingId) { expand(waitingId); waitingId = null; }
    poll.start();
  }
  function closeReports(): void {
    open = false;
    poll.stop();
  }

  function viewFiled(): void {
    const r = filed;
    filed = null;
    if (r) expanded = r.id;
    showReports();
  }

  function expand(id: string | null): void {
    expanded = id;
    if (id) client.seen(id).then(() => badge.now(), () => {});
  }

  async function reply(id: string, body: string): Promise<void> {
    merge(await client.comment(id, body));
  }

  async function verify(id: string): Promise<void> {
    const context = await session.verdictContext();
    merge(await client.verify(id, { context }));
    badge.now();
  }

  /** "Still broken": a note, and the mode bar for a new capture. */
  function stillBroken(r: Report): void {
    closeReports();
    filed = null;
    revokeClip();
    draft = newDraft(null, { kind: "reopen", report: r }, session.diagnosticsDefault);
    const of = draft;
    const begun = session.begin(null);
    of.diagnostics = begun.diagnostics;
    pendingFor = { of, ready: begun.context.then(c => { if (draft === of && !draft.context) draft.context = c; }) };
    pickFor = null;
    bar = { mode: null };
  }

  // ---- The badge ----

  let pulseTimer: unknown = null;
  const badge = untrack(() => new BadgeWatch(() => client.badge(), win as never, {
    changed: n => { count = n; onBadge?.(n); },
    rose: ids => {
      waitingId = ids[0] ?? waitingId;
      timers.clear(pulseTimer);
      pulse = true;
      pulseTimer = timers.set(() => { pulse = false; }, 1600);
    },
  }, timers));
  untrack(() => badge.start());

  const status = $derived(draft ? clipStatus(draft.clip) : null);
  /** What a send would carry now, as "What's sent" shows it (the marks are drawn in at Send). */
  const outgoing = $derived(draft ? (draft.purpose.kind === "reopen" ? reopenOf(draft) : reportOf(draft, { url: pageUrl(doc), title: doc.title }, { externalId })) : null);
  let confirmDrop = $state<{ mode: Mode } | null>(null);
  let noteEl = $state<HTMLTextAreaElement | null>(null);
  const reopening = $derived(draft?.purpose.kind === "reopen" ? draft.purpose.report : null);
</script>

<div class="cx">
  {#if bar}
    <ModeBar mode={bar.mode} {outlined} onMode={choose} onCancel={() => closeBar()} />
  {/if}

  <div class="dock">
  {#if draft && !open}
    <form class="composer" aria-label={reopening ? "Still broken" : "New report"} onsubmit={e => { e.preventDefault(); void send(); }}>
      {#if reopening}
        <p class="quote">Still broken: {summaryOf(reopening)}</p>
      {:else}
        <p class="quote">{draft.anchor ? quoteOf(draft.anchor) : "What the page shows"}</p>
      {/if}
      {#if clipUrl}
        <Annotator src={clipUrl} alt={draft.anchor ? "Screenshot of the picked part of the page" : "Screenshot of the page"} shapes={draft.shapes} scale={draft.capture?.scale ?? 1}
          pick={draft.capture?.pick ?? null} onChange={s => { if (draft) { draft.shapes = s; draft.marksNote = null; } }} />
      {:else if status}
        <p class="muted small" role="status">{status}</p>
      {/if}
      {#if draft.capture?.error}<p class="small warn" role="status">{draft.capture.error}</p>{/if}
      {#if draft.capture?.note}<p class="small warn" role="status">{draft.capture.note}</p>{/if}
      {#if exactSupported && draft.capture && draft.capture.info.method === "dom"}
        <p class="exact small" class:suggested={draft.capture.suggestExact}>
          {#if draft.capture.suggestExact}<span>Part of this didn't render (a frame, canvas or video).</span>{/if}
          <button type="button" class:primary={draft.capture.suggestExact} disabled={exacting} onclick={() => void useExact()}>Use exact screenshot</button>
        </p>
      {/if}
      {#if reopening && !bar}
        <p class="small"><button type="button" class="ghost" onclick={() => { bar = { mode: null }; }}>{draft.clip.state === "none" ? "Add a screenshot" : "Retake the screenshot"}</button></p>
      {/if}
      <textarea rows={3} aria-label={reopening ? "What is still broken" : "Report"} placeholder={reopening ? "What is still broken?" : "What should change here?"} {@attach el => el.focus()} bind:this={noteEl} bind:value={draft.body}
        onkeydown={e => {
          if (e.key === "Escape") { e.preventDefault(); cancel(); }
          else if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) { e.preventDefault(); void send(); }
        }}></textarea>
      <div class="toggles small">
        {#if draft.clip.state !== "none"}
          <label><input type="checkbox" aria-label="Include screenshot" bind:checked={draft.includeClip} /> Include screenshot</label>
        {/if}
        <label><input type="checkbox" aria-label={`Include recent errors (${draft.diagnostics.length})`} disabled={draft.diagnostics.length === 0} bind:checked={draft.includeDiagnostics} /> Include recent errors ({draft.diagnostics.length})</label>
      </div>
      {#if outgoing}<Preview sent={outgoing} marked={draft.shapes.length > 0} />{/if}
      {#if draft.marksNote}<p class="small warn marks" role="status">{draft.marksNote}</p>{/if}
      {#if draft.error}<p class="error small" role="alert">Not sent: {draft.error}</p>{/if}
      <div class="actions">
        <button type="button" onclick={cancel}>Cancel</button>
        <button type="submit" class="primary" disabled={!canSend(draft)}>{draft.sending ? "Sending…" : "Send"}</button>
      </div>
    </form>
  {/if}

  {#if confirmDrop}
    <div class="confirm" role="alertdialog" aria-label="Drop your note?" aria-describedby="cx-drop-text">
      <p id="cx-drop-text">Start a new report? Your "Still broken" note and its screenshot will be dropped.</p>
      <div class="actions">
        <button type="button" {@attach el => el.focus()} onclick={() => { confirmDrop = null; queueMicrotask(() => noteEl?.focus()); }}>Keep writing</button>
        <button type="button" class="primary" onclick={() => { const m = confirmDrop?.mode ?? "element"; draft = null; startReport(m); }}>Drop it</button>
      </div>
    </div>
  {/if}

  {#if filed && !open && !draft}
    <section class="filed" role="status" aria-label="Filed">
      <p><strong>Filed.</strong> Replies and fixes show in Your reports.</p>
      <div class="actions">
        <button type="button" class="ghost" aria-label="Dismiss" onclick={() => { filed = null; }}>✕</button>
        <button type="button" class="primary" bind:this={viewBtn} onclick={viewFiled}>View in Your reports</button>
      </div>
    </section>
  {/if}

  {#if open}
    <Reports {reports} error={loadError} {expanded} onExpand={expand} onReply={reply} onVerify={verify} onStillBroken={stillBroken} onClose={closeReports} />
  {/if}

  {#if notice}<p class="notice" role="status">{notice}</p>{/if}

  {#if launcher === "corner"}
    <div class="launcher" class:pulse>
      <button type="button" class="reports-btn" aria-expanded={open} onclick={toggleReports}>Your reports{#if count}{" "}<span class="count" aria-label={`${count} waiting`}>{count}</span>{/if}</button>
      {#if canFile}
        <button type="button" class="primary report-btn" bind:this={reportBtn} aria-pressed={picking} onclick={() => (picking ? closeBar() : report())}>
          <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M3 2.5h10a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 11.5H7l-3.5 2.5v-2.5H3A1.5 1.5 0 0 1 1.5 10V4A1.5 1.5 0 0 1 3 2.5z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>
          {picking ? "Picking…" : "Report"}
        </button>
      {/if}
    </div>
  {/if}
  </div>
</div>

<style>
  .cx { pointer-events: auto; }
  .dock { position: fixed; right: 16px; bottom: 16px; display: flex; flex-direction: column; align-items: flex-end; gap: 8px; max-height: calc(100vh - 32px); pointer-events: none; }
  .dock > :global(*) { flex: 0 1 auto; min-height: 0; pointer-events: auto; }
  .launcher { display: flex; flex: none; gap: 6px; }
  .launcher button { box-shadow: var(--elev-lg); border-radius: 999px; padding: 7px 14px; }
  .launcher .reports-btn { background: var(--raised); border-color: var(--border-hover); }
  .launcher .reports-btn[aria-expanded="true"] { background: var(--hover); }
  .launcher .report-btn[aria-pressed="true"] { background: var(--you); color: var(--on-you); border-color: var(--you); }
  .count { min-width: 18px; padding: 1px 5px; border-radius: 999px; background: var(--you); color: var(--on-you); font-size: 12px; line-height: 16px; text-align: center; }
  .launcher.pulse .reports-btn { animation: pulse .8s ease-out 1; }
  @keyframes pulse { 0% { box-shadow: 0 0 0 0 var(--you); } 100% { box-shadow: 0 0 0 12px transparent; } }
  @media (prefers-reduced-motion: reduce) { .launcher.pulse .reports-btn { animation: none; outline: 2px solid var(--you); } }
  .composer { width: min(380px, calc(100vw - 32px)); overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius); padding: 12px; box-shadow: var(--elev-lg); z-index: 2; }
  .quote { margin: 0 0 8px; padding: 4px 8px; font-size: 13px; color: var(--muted); background: var(--comment-hl); border-left: 2px solid var(--you); border-radius: 0 var(--radius-xs) var(--radius-xs) 0; overflow-wrap: anywhere; }
  .composer > p[role="status"] { margin-bottom: 8px; }
  .warn { margin-bottom: 6px; color: var(--you-ink); }
  .exact { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; margin-bottom: 8px; }
  .exact button { min-height: 26px; padding: 3px 10px; font-size: 12px; }
  .toggles { display: flex; flex-wrap: wrap; gap: 4px 14px; margin-top: 8px; }
  .toggles label { display: inline-flex; gap: 6px; align-items: center; }
  .actions { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; }
  .error { margin-top: 6px; color: var(--danger); }
  .confirm { flex: none; max-width: min(360px, calc(100vw - 32px)); padding: 10px 12px; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius-sm); box-shadow: var(--elev-lg); font-size: 13px; }
  .notice, .filed { flex: none; margin: 0; max-width: min(360px, calc(100vw - 32px)); padding: 8px 12px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius-sm); box-shadow: var(--elev-lg); font-size: 13px; }
  .filed { border-top: 3px solid var(--agent-ink); }
  .filed .actions { margin-top: 6px; }
  @media (max-width: 480px) {
    .composer { position: fixed; left: 0; right: 0; bottom: 0; width: auto; max-height: 70vh; border-width: 3px 0 0; border-radius: var(--radius) var(--radius) 0 0; z-index: 4; }
  }
</style>
