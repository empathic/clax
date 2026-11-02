<script lang="ts">
  // The overlay a deployer's page embeds (`Clax.overlay`): a launcher that
  // starts comment mode, a composer for the pick (text, the clip's preview,
  // Send and Cancel) and "Your reports" with their replies, polled every
  // POLL_MS only while it is open. A clip that cannot be drawn leaves the
  // report without one; the send waits for a clip still being taken.
  // "Your reports" opened over a draft hides the composer and keeps the
  // draft, which comes back when the panel closes. The composer or the
  // panel, the notices zone and the launcher stack up from the bottom right
  // corner, none over another; closing the composer gives focus back to
  // Report. Every notice, status and failure is said in the notices zone
  // alone (a region and a polite live region), which is always there and
  // always one size ("No notices" when empty), so none moves another part.
  // On mount it reads "Your reports" once (`check`), before anyone clicks:
  // a refusal of the queue or the instance (`problemOf`) shows as a polite
  // notice that takes no focus, says what is wrong and offers what fixes
  // it here: Try again when Clax could not be reached, and, on a page of
  // the instance's own origin whose viewer is the queue's admin (`admin`,
  // their own Clax session), a button that adds the page's origin to the
  // queue. Dismissed, a problem stays away for the page load unless the
  // person's own send or reply meets it again.
  import { onDestroy, untrack } from "svelte";
  import type { Report } from "../../sdk/src/types";
  import { type AdminQueue, type Clip, type Draft, POLL_MS, Poll, type QueueAdminApi, type QueueProblem, type ReporterApi, type Timers, canSend, clipStatus, failureText, listsOrigin, newDraft, pageOrigin, pageUrl, problemOf, quoteOf, realTimers, reportOf, takeClip, withOrigin } from "./model";
  import type { Picked, Picker } from "./picker";
  import Reports from "./Reports.svelte";

  type Props = {
    client: ReporterApi;
    doc: Document;
    /** The overlay's host element: while comment mode is on, presses on it
     * are kept from comment mode, and a click on one of its buttons is
     * passed on to that button. */
    host: Element;
    picker: (onPick: (p: Picked) => void, onCancel: () => void) => Picker;
    externalId?: string;
    metadata?: unknown;
    timers?: Timers;
    /** The queue's admin routes with the viewer's own session; `null` off
     * the instance's origin. */
    admin?: QueueAdminApi | null;
    /** Reads the queue once on mount. */
    check?: boolean;
  };
  let { client, doc, host, picker: makePicker, externalId, metadata, timers = realTimers, admin = null, check: checkOnMount = true }: Props = $props();
  const origin = untrack(() => pageOrigin(doc));

  let problem = $state<QueueProblem | null>(null);
  /** The queue, when the viewer is its admin and the problem may be its origins. */
  let adminQueue = $state<AdminQueue | null>(null);
  let fixing = $state(false);
  let fixError = $state<string | null>(null);
  /** The problems shown this page load, by code. */
  const shown = new Set<string>();

  /** Shows `e`'s problem, if it is one: one the page met by itself
   * (`prompted` false) only if it was not shown yet. */
  function raise(e: unknown, prompted: boolean): void {
    const p = problemOf(e, origin);
    if (!p || problem?.code === p.code || (!prompted && shown.has(p.code))) return;
    shown.add(p.code);
    problem = p;
    adminQueue = null;
    fixError = null;
    const code = p.code;
    if (p.addOrigin && admin) {
      admin.queue().then(q => { if (problem?.code === code) adminQueue = q; }, () => {});
    }
  }

  /** Reads the queue; a refusal shows as a problem, and a read that holds clears it. */
  async function check(prompted: boolean): Promise<boolean> {
    try {
      await client.myReports();
      problem = null;
      return true;
    } catch (e) {
      // A check that fails some other way (the page's `auth` threw, or the
      // browser cannot sign) still says so.
      raise(problemOf(e, origin) ? e : { code: "check_failed", message: failureText(e) }, prompted);
      return false;
    }
  }
  if (untrack(() => checkOnMount)) void check(false);

  /** Adds the page's origin to the queue, then reads it again. */
  async function allow(): Promise<void> {
    const q = adminQueue;
    if (!q || !admin || fixing) return;
    fixing = true;
    fixError = null;
    try {
      adminQueue = await admin.setOrigins(withOrigin(q.origins, origin));
      if (await check(true)) say(`${origin} may now report into ${q.name}.`);
    } catch (e) {
      fixError = `Not changed: ${failureText(e)}`;
    } finally {
      fixing = false;
    }
  }

  const fixText = $derived.by(() => {
    if (!problem) return null;
    if (problem.addOrigin && adminQueue) {
      return listsOrigin(adminQueue.origins, origin)
        ? `${adminQueue.name} lists ${origin} already, so this page's reporter token was made for another origin. Whoever runs this site has to fix how its page gets one.`
        : `You are an admin of ${adminQueue.name}.`;
    }
    return problem.fix;
  });
  const canAllow = $derived(!!problem?.addOrigin && !!adminQueue && !listsOrigin(adminQueue.origins, origin));

  let picking = $state(false);
  let draft = $state<Draft | null>(null);
  let clipUrl = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let open = $state(false);
  let reports = $state<Report[] | null>(null);
  let loadError = $state<string | null>(null);
  let reportBtn = $state<HTMLButtonElement | null>(null);

  /** The draft's clip, for a send to wait on; keyed by the draft it is for. */
  let clipFor: { of: Draft; clip: Promise<Clip> } | null = null;

  const picker = untrack(() => makePicker(onPick, () => stopPicking()));
  onDestroy(() => { picker.destroy(); poll.stop(); revokeClip(); timers.clear(noticeTimer); });

  function startPicking(): void {
    if (open) toggleReports();
    draft = null;
    revokeClip();
    notice = null;
    picking = true;
    picker.set(true);
  }
  function stopPicking(): void {
    picking = false;
    picker.set(false);
  }

  // Comment mode takes every press and click on the page, the overlay's
  // own included: while it is on, those on the overlay stop at the window,
  // and a click on one of its buttons is given to the button as a click
  // comment mode does not take (one the page did not get from the viewer).
  $effect(() => {
    if (!picking) return;
    const win = doc.defaultView;
    if (!win) return;
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

  function onPick(p: Picked): void {
    picking = false;
    // Comment mode ends once its hook has returned.
    queueMicrotask(() => picker.set(false));
    revokeClip();
    const d = newDraft(p.anchor);
    draft = d;
    const of = draft;
    const clip = takeClip(() => p.render()).then(c => {
      picker.done();
      if (draft === of) {
        draft.clip = c;
        if (c.state === "taken") clipUrl = URL.createObjectURL(c.png);
      }
      return c;
    });
    clipFor = { of, clip };
  }

  function revokeClip(): void {
    if (clipUrl) URL.revokeObjectURL(clipUrl);
    clipUrl = null;
  }

  /** Drops the draft and gives focus back to the Report button. */
  function cancel(): void {
    draft = null;
    clipFor = null;
    revokeClip();
    reportBtn?.focus();
  }

  let noticeTimer: unknown = null;
  function say(text: string): void {
    timers.clear(noticeTimer);
    notice = text;
    noticeTimer = timers.set(() => { notice = null; }, 6000);
  }

  async function send(): Promise<void> {
    const d = draft;
    if (!d || !canSend(d)) return;
    d.sending = true;
    d.error = null;
    try {
      if (clipFor?.of === d) d.clip = await clipFor.clip;
      const report = await client.fileReport(reportOf(d, { url: pageUrl(doc), title: doc.title }, { externalId, metadata }));
      problem = null;
      if (draft !== d) return;
      cancel();
      if (reports) reports = [report, ...reports.filter(r => r.id !== report.id)];
      say("Report sent. Replies show under Your reports.");
    } catch (e) {
      d.error = failureText(e, origin);
      raise(e, true);
    } finally {
      d.sending = false;
    }
  }

  const poll = untrack(() => new Poll(async () => {
    const page = await client.myReports();
    problem = null;
    if (!open) return;
    reports = page.reports;
    loadError = null;
  }, timers, POLL_MS, e => { loadError = failureText(e, origin); raise(e, false); }));

  function toggleReports(): void {
    open = !open;
    replyError = null;
    if (open) poll.start();
    else poll.stop();
  }

  async function reply(id: string, body: string): Promise<void> {
    let r: Report;
    try {
      r = await client.comment(id, body);
    } catch (e) {
      raise(e, true);
      throw e;
    }
    if (reports) reports = reports.map(x => (x.id === r.id ? r : x));
  }

  const status = $derived(draft && !open ? clipStatus(draft.clip) : null);
  /** Why the last reply on a report was not sent. */
  let replyError = $state<string | null>(null);
  /** The failures the zone says besides the problem, each once and not
   * again when the problem says the same. */
  const errors = $derived([
    draft && !open && draft.error ? `Not sent: ${draft.error}` : null,
    open && loadError && loadError !== problem?.text ? loadError : null,
    open && replyError ? `Reply not sent: ${replyError}` : null,
  ].filter((e): e is string => e !== null && e !== `Not sent: ${problem?.text}`));
</script>

<div class="cx">
  {#if picking}
    <div class="hint" role="status">
      <span>Click what your report is about, select text, or drag over an area. Esc cancels.</span>
      <button type="button" onclick={stopPicking}>Cancel</button>
    </div>
  {/if}

  <div class="dock">
  {#if draft && !open}
    <form class="composer" aria-label="New report" onsubmit={e => { e.preventDefault(); void send(); }}>
      <p class="quote">{quoteOf(draft.anchor)}</p>
      {#if clipUrl}
        <figure class="clip">
          <img src={clipUrl} alt="Screenshot of the picked part of the page" />
          <figcaption class="small muted">Screenshot</figcaption>
        </figure>
      {/if}
      <textarea rows={3} aria-label="Report" placeholder="What should change here?" {@attach el => el.focus()} bind:value={draft.body}
        onkeydown={e => {
          if (e.key === "Escape") { e.preventDefault(); cancel(); }
          else if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) { e.preventDefault(); void send(); }
        }}></textarea>
      <div class="actions">
        <button type="button" onclick={cancel}>Cancel</button>
        <button type="submit" class="primary" disabled={!canSend(draft)}>{draft.sending ? "Sending…" : "Send"}</button>
      </div>
    </form>
  {/if}

  {#if open}
    <Reports reports={reports} error={loadError} {origin} onReply={reply} onReplyError={t => { replyError = t; }} onClose={toggleReports} />
  {/if}

  <!-- The notices' own zone: always there and always this size, so no
       notice moves the composer, the panel or the launcher. -->
  <section class="zone" aria-label="Clax notices">
    <div class="live" role="status">
      {#if problem}
        <div class="problem">
          <p>{problem.text}</p>
          {#if fixText}<p class="small">{fixText}</p>{/if}
          {#if fixError}<p class="small error">{fixError}</p>{/if}
        </div>
      {/if}
      {#each errors as e (e)}<p class="small error">{e}</p>{/each}
      {#if status}<p class="small muted">{status}</p>{/if}
      {#if notice}<p class="notice small">{notice}</p>{/if}
      {#if !problem && errors.length === 0 && !status && !notice}<p class="none small muted">No notices</p>{/if}
    </div>
    {#if problem}
      <div class="actions">
        {#if canAllow && adminQueue}
          <button type="button" class="primary" disabled={fixing} onclick={() => void allow()}>{fixing ? "Allowing…" : `Let ${origin} report into ${adminQueue.name}`}</button>
        {/if}
        {#if problem.retry}<button type="button" onclick={() => void check(true)}>Try again</button>{/if}
        <button type="button" onclick={() => { problem = null; }}>Dismiss</button>
      </div>
    {/if}
  </section>

  <div class="launcher">
    <button type="button" class="reports-btn" aria-expanded={open} onclick={toggleReports}>Your reports</button>
    <button type="button" class="primary report-btn" bind:this={reportBtn} aria-pressed={picking} onclick={() => (picking ? stopPicking() : startPicking())}>
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M3 2.5h10a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 13 11.5H7l-3.5 2.5v-2.5H3A1.5 1.5 0 0 1 1.5 10V4A1.5 1.5 0 0 1 3 2.5z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>
      {picking ? "Picking…" : "Report"}
    </button>
  </div>
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
  .hint { position: fixed; left: 50%; top: 12px; transform: translateX(-50%); max-width: calc(100vw - 32px); display: flex; gap: 10px; align-items: center; padding: 6px 6px 6px 14px; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius); box-shadow: var(--elev-lg); font-size: 13px; z-index: 3; }
  .composer { width: min(360px, calc(100vw - 32px)); overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-top: 3px solid var(--you); border-radius: var(--radius); padding: 12px; box-shadow: var(--elev-lg); z-index: 2; }
  .quote { margin: 0 0 8px; padding: 4px 8px; font-size: 13px; color: var(--muted); background: var(--comment-hl); border-left: 2px solid var(--you); border-radius: 0 var(--radius-xs) var(--radius-xs) 0; overflow-wrap: anywhere; }
  .clip { margin: 0 0 8px; padding: 4px; border: 1px solid var(--border-strong); border-radius: var(--radius-xs); background: var(--bg); }
  .clip img { display: block; max-width: 100%; min-height: 24px; max-height: 160px; object-fit: cover; object-position: left top; }
  .clip figcaption { margin-top: 2px; }
  .actions { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; }
  .error { margin-top: 6px; color: var(--danger); }
  /* A fixed size whatever it says: a longer notice scrolls inside it. */
  .zone { flex: none; box-sizing: border-box; width: min(360px, calc(100vw - 32px)); height: 112px; overflow-y: auto; padding: 8px 12px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius-sm); box-shadow: var(--elev-lg); font-size: 13px; overflow-wrap: anywhere; }
  .zone p { margin: 0 0 4px; }
  .zone .problem { padding-left: 8px; border-left: 3px solid var(--danger); }
  .zone .none { margin: 0; }
  .zone .actions { justify-content: flex-start; flex-wrap: wrap; margin-top: 4px; }
  @media (max-width: 480px) {
    .composer { position: fixed; left: 0; right: 0; bottom: 0; width: auto; max-height: 70vh; border-width: 3px 0 0; border-radius: var(--radius) var(--radius) 0 0; z-index: 4; }
  }
</style>
