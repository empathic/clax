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
  //
  // Every feedback feature is a plugin (sdk/src/plugin.ts, `plugins`): the
  // capture methods, the Annotator and its tools, the privacy filters (run
  // on each capture before the reporter sees it), the attachments (each its
  // own box and its own line in "What's sent"), the modes of the mode bar
  // and what a pick in code records, and how the composer is presented
  // (the corner dock, a dialog, a drawer, a sheet, its own window, or handed
  // to the Clax extension). The overlay renders none of them unless a plugin
  // is in. "What's sent" stays the overlay's own: it is where the core
  // refuses data that comes without a line in it.
  import { mount as mountComponent, onDestroy, onMount, unmount as unmountComponent, untrack, type Component, type Snippet } from "svelte";
  import { addSheet } from "../../bridge/src/sheet";
  import type { AttachmentData, CaptureContribution, PluginHost, SentRow } from "../../sdk/src/plugin";
  import type { DiagLine, ReportContext, Report } from "../../sdk/src/types";
  import { type FlattenDeps, type Shape, browserFlatten, flatten } from "./annotate";
  import type { CaptureDeps, Captured, Mode } from "./capture";
  import Portal from "./Portal.svelte";
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
    /** The plugins: every feature the overlay shows comes from one. */
    plugins: PluginHost;
    /** The presentation asked for (a presentation plugin's id); the first available one otherwise. */
    presentation?: string;
    /** The overlay's stylesheet, for a composer drawn in its own window. */
    css?: string;
    /** Opens the composer's window (tests replace it). */
    openWindow?: (win: Window) => Window | null;
    /** Turned on or off by config, over each plugin's own default (filter and attachment ids). */
    defaults?: Record<string, boolean>;
  };
  let {
    client, doc, host, picker: makePicker, session, externalId, launcher = "corner", canFile = true, capture: captureDeps = {}, flatten: flattenDeps = browserFlatten,
    timers = realTimers, onBadge, onFiled, plugins, presentation: presentationId, css = "", defaults = {},
    openWindow = (w: Window) => w.open("", "clax-composer", "popup,width=480,height=760"),
  }: Props = $props();

  const win = untrack(() => doc.defaultView ?? window);

  // ---- Plugins ----

  const domCapture = untrack(() => plugins.list("capture").find((c: CaptureContribution) => c.method === "dom") ?? null);
  const displayCapture = untrack(() => plugins.list("capture").find((c: CaptureContribution) => c.method === "display") ?? null);
  const AnnotatorUi = untrack(() => (plugins.annotateUi() ?? null) as Component<Record<string, unknown>> | null);
  const tools = untrack(() => plugins.tools());
  const filters = untrack(() => plugins.list("filters"));
  const attachments = untrack(() => plugins.list("attachments"));
  /** The mode bar's modes: the pickers that are comment modes, in plugin order. */
  const barModes = untrack(() => plugins.list("pickers").filter(p => p.mode).map(p => ({ mode: p.mode as Mode, label: p.label, key: (p.key ?? p.mode![0]).toUpperCase() })));
  const pres = untrack(() => plugins.presentation(presentationId, win));
  const layout = untrack(() => pres?.layout ?? "dock");
  const onDefault = (id: string, d: boolean) => (typeof defaults[id] === "boolean" ? defaults[id] : d);
  /** The filters on, and the attachments ticked, for the draft being written. */
  let filtersOn = $state<string[]>([]);
  let attachOn = $state<string[]>([]);
  /** What the filters did to the screenshot ("Code blurred (2)"). */
  let filterNotes = $state<string[]>([]);
  /** The attachments' lines for "What's sent", read when the draft started. */
  let attachRows = $state<SentRow[]>([]);
  /** What the pickers' refinements said of the pick (`context.app` keys). */
  let refined: Record<string, unknown> = {};
  /** A draft's screenshot as it was taken, before the filters ran. */
  let rawCapture = $state.raw<Captured | null>(null);

  /** Resets the plugins' part of a new draft: the filters and attachments to their defaults, and reads the attachments' lines. */
  function pluginDraft(): void {
    filtersOn = filters.filter(f => onDefault(f.id, f.default)).map(f => f.id);
    attachOn = attachments.filter(a => onDefault(a.id, a.default)).map(a => a.id);
    filterNotes = [];
    refined = {};
    rawCapture = null;
    attachRows = [];
    if (attachments.length) void plugins.attach(attachments.map(a => a.id), doc).then(r => { attachRows = r.rows; });
  }

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
  const exactSupported = untrack(() => !!displayCapture && (captureDeps.supported ?? displayCapture.supported?.(win) ?? true));

  /** What the keyboard has outlined, for the mode bar to announce. */
  let outlined = $state<string | null>(null);
  // Comment mode is made once the overlay mounts, with its teardown: an
  // overlay unmounted before its effects ran (closed in the task that
  // started it) never makes it, so leaves none of it in the page.
  let picker: Picker | null = null;
  onMount(() => {
    const made = makePicker(onPick, () => closeBar(), what => { outlined = what; });
    picker = made;
    return () => made.destroy();
  });
  onDestroy(() => {
    closeWindow();
    poll.stop();
    badge.stop();
    revokeClip();
    timers.clear(noticeTimer);
    timers.clear(pulseTimer);
  });

  // ---- The mode bar ----

  /** Starts a new report in `mode` (Element by default). A "Still broken"
   * note being written is not dropped unasked: the reporter is asked first. */
  export function report(opts: { mode?: Mode | "note" } = {}): void {
    if (!canFile) return;
    const mode = opts.mode ?? barModes[0]?.mode ?? "note";
    // A presentation outside the page (the Clax extension) takes the report whole.
    if (pres?.handoff?.({ mode, url: pageUrl(doc), title: doc.title }, win)) return;
    if (draft?.purpose.kind === "reopen" && (draft.body.trim() !== "" || draft.clip.state !== "none")) {
      confirmDrop = { mode };
      return;
    }
    // Its own window opens now, in the click or key that asked for it (a
    // popup blocker allows only that); the composer is drawn there once there is one.
    if (layout === "window") openComposerWindow();
    startReport(mode);
  }

  function startReport(mode: Mode | "note"): void {
    confirmDrop = null;
    if (open) closeReports();
    draft = null;
    filed = null;
    revokeClip();
    notice = null;
    pluginDraft();
    // Note first, or no way to pick or capture: the composer at once, about the whole page.
    if (mode === "note" || !barModes.some(m => m.mode === mode)) {
      draft = newDraft(null, { kind: "new" }, session.diagnosticsDefault);
      draft.clip = { state: "none" };
      const of = draft;
      const begun = session.begin(null);
      of.diagnostics = begun.diagnostics;
      pendingFor = { of, ready: Promise.all([begun.context.then(c => { if (draft === of) draft.context = c; }), begun.metadata.then(m => { if (draft === of) draft.metadata = m; })]) };
      pickFor = null;
      return;
    }
    bar = { mode: null };
    choose(mode);
  }

  // ---- Its own window ----

  let popup: Window | null = null;
  let portal: ReturnType<typeof mountComponent> | null = null;
  /** The composer's window failed to open (blocked): the dialog stands in. */
  let windowRefused = $state(false);

  function openComposerWindow(): void {
    if (popup && !popup.closed) { popup.focus?.(); return; }
    let w: Window | null = null;
    try { w = openWindow(win); } catch { w = null; }
    windowRefused = !w;
    popup = w;
    if (!w) return;
    const d = w.document;
    d.title = "Send feedback";
    d.documentElement.lang = doc.documentElement.lang || "en";
    addSheet(d, `${css.replaceAll(":host", ":root")}\nhtml,body{margin:0;background:var(--bg)}`);
    const target = d.createElement("div");
    target.className = "cx window";
    d.body.append(target);
    portal = mountComponent(Portal, { target, props: { content: composerSnippet } });
    // Closing the window drops the draft, as Cancel does.
    w.addEventListener("pagehide", () => { if (popup === w) { popup = null; portal = null; if (draft) cancel(); } });
  }

  function closeWindow(): void {
    const w = popup;
    popup = null;
    if (portal) { void unmountComponent(portal); portal = null; }
    try { w?.close(); } catch { /* already gone */ }
  }

  // The composer's window closes once its draft is sent or dropped.
  $effect(() => { if (!draft && popup && !bar) closeWindow(); });

  /** Opens "Your reports". */
  export function showReports(): void {
    if (!open) toggleReports();
  }

  /** Chooses `mode` in the mode bar. Element chosen again while it is on
   * (E, or its button) starts picking from the keyboard: focus leaves the
   * overlay, so the arrow keys move the outline and Enter picks. */
  function choose(mode: Mode): void {
    if (!bar) return;
    if (mode === "element" && bar.mode === "element" && picking) {
      picker?.keyboard();
      const root = cx?.getRootNode() as Document | ShadowRoot | undefined;
      const at = root?.activeElement;
      if (at instanceof HTMLElement && cx?.contains(at)) at.blur();
      return;
    }
    bar.mode = mode;
    if (mode === "screen") {
      setPicking(false);
      startCapture("screen", null);
      return;
    }
    picker?.areasOnly(mode === "area");
    setPicking(true);
  }

  function setPicking(on: boolean): void {
    picking = on;
    picker?.set(on);
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
    queueMicrotask(() => picker?.set(false));
    const mode = bar?.mode === "area" ? "area" : "element";
    const veto = plugins.emit("pick", { mode, target: p.target ?? null }).cancelled;
    if (veto) { picker?.done(); closeBar(); say(veto); return; }
    startCapture(mode, p);
  }

  /** Captures `mode` (of pick `p`), into the draft being written (a reopen,
   * or a note that asked for a screenshot) or a new draft. Without a capture
   * plugin the draft takes the pick alone. */
  function startCapture(mode: Mode, p: Picked | null): void {
    bar = null;
    revokeClip();
    let d: Draft;
    if (draft && (draft.purpose.kind === "reopen" || adding)) {
      d = draft;
      d.anchor = draft.purpose.kind === "reopen" || mode === "screen" || !p ? null : p.anchor;
      d.capture = null;
      d.clip = domCapture ? { state: "taking" } : { state: "none" };
      d.shapes = [];
    } else {
      draft = newDraft(mode === "screen" ? null : p!.anchor, { kind: "new" }, session.diagnosticsDefault);
      if (!domCapture) draft.clip = { state: "none" };
      d = draft;
    }
    adding = false;
    rawCapture = null;
    refined = plugins.refine(p?.target ?? null, doc);
    const of = d;
    const begun = session.begin({ mode: mode === "screen" || !p ? "screen" : p.mode, method: "dom" });
    of.diagnostics = begun.diagnostics;
    pickFor = { of, mode, pick: p };
    const context = begun.context.then(c => { if (draft === of) draft.context = draft.capture ? withCapture(c, draft.capture.info) : c; });
    const metadata = begun.metadata.then(m => { if (draft === of) draft.metadata = m; });
    const taken = domCapture
      ? (domCapture.take({ mode, pick: p, win, deps: captureDeps }) as Promise<Captured>).then(c => {
        if (p) picker?.done();
        if (draft === of) return applyCapture(c);
      })
      : Promise.resolve(p ? picker?.done() : undefined);
    pendingFor = { of, ready: Promise.all([context, metadata, taken]) };
  }

  /** The dialog keeps focus while it is open (aria-modal): Tab and Shift+Tab
   * wrap within it, and Escape anywhere in it drops the draft, as Close does. */
  function trapFocus(e: KeyboardEvent): void {
    const frame = e.currentTarget as HTMLElement;
    if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); cancel(); return; }
    if (e.key !== "Tab") return;
    const all = [...frame.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), textarea, summary, [tabindex='0']")];
    if (!all.length) return;
    const root = frame.getRootNode() as Document | ShadowRoot;
    const at = root.activeElement;
    const first = all[0], last = all[all.length - 1];
    if (e.shiftKey && (at === first || at === frame)) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && at === last) { e.preventDefault(); first.focus(); }
  }

  /** "Add a screenshot" on a draft that has none: the mode bar, for a capture into it. */
  let adding = false;
  function addScreenshot(): void {
    adding = draft?.purpose.kind === "new";
    bar = { mode: null };
  }

  /** Puts capture `c` in the draft, through the filters that are on.
   * Replacing the page render with the exact screenshot (`carry`) moves the
   * reporter's marks onto it; a capture that fell back to the render it
   * replaces keeps them as they are. */
  async function applyCapture(c: Captured, carry = false): Promise<void> {
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
    rawCapture = c;
    draft.capture = c;
    if (draft.context) draft.context = withCapture(draft.context, c.info);
    await filterInto(draft, c);
  }

  /** Runs the filters that are on over capture `c` into `d`'s clip. A
   * filter that fails leaves the screenshot out, never unfiltered. */
  let filterRun = 0;
  async function filterInto(d: Draft, c: Captured): Promise<void> {
    const run = ++filterRun;
    filterNotes = [];
    const clip = clipOf(c);
    if (clip.state !== "taken" || !filters.length || !filtersOn.length || !c.at) {
      d.clip = clip;
      if (clip.state === "taken") { revokeClip(); clipUrl = URL.createObjectURL(clip.png); }
      return;
    }
    d.clip = { state: "taking" };
    try {
      const out = await plugins.filter({ png: clip.png, at: c.at, scale: c.scale, doc }, $state.snapshot(filtersOn) as string[]);
      // A later capture, toggle or draft has taken over.
      if (run !== filterRun || draft !== d) return;
      filterNotes = out.notes.map(n => n.note);
      d.clip = { state: "taken", png: out.png };
      revokeClip();
      clipUrl = URL.createObjectURL(out.png);
    } catch (e) {
      if (run === filterRun && draft === d) d.clip = { state: "failed", error: failureText(e) };
    }
  }

  /** A filter turned on or off: the screenshot is filtered again from the one taken. */
  function toggleFilter(id: string, on: boolean): void {
    filtersOn = on ? [...filtersOn.filter(x => x !== id), id] : filtersOn.filter(x => x !== id);
    const d = draft, c = rawCapture;
    if (d && c) {
      const ready = filterInto(d, c);
      pendingFor = { of: d, ready: Promise.all([pendingFor?.of === d ? pendingFor.ready : null, ready]) };
    }
  }

  let exacting = $state(false);
  /** The browser's own screenshot of the tab, in place of the page render. */
  async function useExact(): Promise<void> {
    const d = draft;
    const from = pickFor;
    if (!d || !from || from.of !== d || exacting || !displayCapture) return;
    exacting = true;
    try {
      // The overlay steps aside with the page's data-clax-ignore elements (capture.ts).
      const c = await (displayCapture.take({ mode: from.mode, pick: from.pick, win, deps: { ...captureDeps, host, fallback: rawCapture ?? d.capture ?? undefined } }) as Promise<Captured>);
      if (draft === d) await applyCapture(c, true);
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

  /** What the pickers and the ticked attachments add to `ctx`, on its own
   * fields: the release, the environment, the app's keys. With a browser
   * attachment unticked, the browser's keys are left out. */
  function withAttached(ctx: ReportContext | undefined, data: AttachmentData): ReportContext {
    const out: ReportContext = ctx ? { ...ctx } : { v: 1, captured_at: new Date().toISOString() };
    if (data.release) out.release = { ...out.release, ...data.release };
    const env = { ...out.environment, ...data.environment };
    if (attachments.some(a => a.id === "browser") && !attachOn.includes("browser")) for (const k of ["user_agent", "platform", "locale", "time_zone"] as const) delete env[k];
    if (out.environment || data.environment) out.environment = env;
    const app = { ...refined, ...data.app };
    if (Object.keys(app).length) out.app = out.app && typeof out.app === "object" && !Array.isArray(out.app) ? { ...(out.app as Record<string, unknown>), ...app } : out.app === undefined ? app : out.app;
    return out;
  }

  /** Whether the plugins add anything to a report. */
  const pluginsAdd = untrack(() => attachments.length > 0 || plugins.list("pickers").some(p => p.refine));

  async function send(): Promise<void> {
    const d = draft;
    if (!d || !canSend(d)) return;
    d.sending = true;
    d.error = null;
    try {
      if (pendingFor?.of === d) await pendingFor.ready;
      const attached = pluginsAdd ? await plugins.attach($state.snapshot(attachOn) as string[], doc) : null;
      const veto = plugins.emit("before-send", { body: d.body, rows: attached?.rows ?? [] }).cancelled;
      if (veto) throw new Error(veto);
      /** The report as the plugins add to it (never the draft itself, so a retry adds nothing twice). */
      const extended = <T extends { context?: ReportContext; diagnostics?: DiagLine[] }>(r: T): T => {
        if (!pluginsAdd) return r;
        const data = attached?.data ?? {};
        const lines = [...(r.diagnostics ?? []), ...(data.diagnostics ?? [])];
        return { ...r, context: withAttached(r.context, data), ...(lines.length ? { diagnostics: lines } : {}) };
      };
      let marked: Blob | undefined;
      try {
        marked = await flattened(d);
      } catch (e) {
        // Never the unmarked clip: its redactions would be lost.
        throw new Error(`your marks could not be drawn into the screenshot (${failureText(e)}); clear them or untick Include screenshot`, { cause: e });
      }
      const purpose = d.purpose;
      if (purpose.kind === "reopen") {
        const r = await client.reopen(purpose.report.id, extended(reopenOf(d, marked)));
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
      const r = await client.fileReport(extended(reportOf(d, { url: pageUrl(doc), title: doc.title }, { externalId }, marked)));
      if (draft !== d) return;
      draft = null;
      revokeClip();
      merge(r, true);
      filed = r;
      onFiled?.(r);
      plugins.emit("sent", { id: r.id });
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
    pluginDraft();
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
  let confirmDrop = $state<{ mode: Mode | "note" } | null>(null);
  /** The composer is drawn in its own window, not the dock. */
  const inWindow = $derived(layout === "window" && !windowRefused);
  /** The dock's layout: the presentation's, or a dialog standing in for a window that could not open. */
  const dockLayout = $derived(layout === "window" ? "dialog" : layout);
  let noteEl = $state<HTMLTextAreaElement | null>(null);
  /** The overlay's own root element. */
  let cx = $state<HTMLDivElement | null>(null);
  const reopening = $derived(draft?.purpose.kind === "reopen" ? draft.purpose.report : null);
  const dialogTitle = $derived(reopening ? "Still broken" : "Send feedback");
</script>

{#snippet composerSnippet()}
  {#if draft && !open}
    <form class="composer" class:framed={dockLayout !== "dock" || inWindow} aria-label={reopening ? "Still broken" : "New report"} onsubmit={e => { e.preventDefault(); void send(); }}>
      {#if dockLayout !== "dock" || inWindow}
        <div class="head">
          <h2 id="cx-composer-title">{dialogTitle}</h2>
          <button type="button" class="ghost small" aria-keyshortcuts="Escape" onclick={cancel}>Close · Esc</button>
        </div>
      {/if}
      {#if reopening}
        <p class="quote">Still broken: {summaryOf(reopening)}</p>
      {:else}
        <p class="quote">{draft.anchor ? quoteOf(draft.anchor) : "What the page shows"}</p>
      {/if}
      {#if clipUrl && AnnotatorUi}
        <AnnotatorUi src={clipUrl} alt={draft.anchor ? "Screenshot of the picked part of the page" : "Screenshot of the page"} shapes={draft.shapes} scale={draft.capture?.scale ?? 1}
          pick={draft.capture?.pick ?? null} {tools} onChange={(s: Shape[]) => { if (draft) { draft.shapes = s; draft.marksNote = null; } }} />
      {:else if clipUrl}
        <img class="shot" src={clipUrl} alt={draft.anchor ? "Screenshot of the picked part of the page" : "Screenshot of the page"} />
      {:else if status}
        <p class="muted small" role="status">{status}</p>
      {/if}
      {#each filterNotes as n (n)}<p class="small muted" role="status">{n}</p>{/each}
      {#if draft.capture?.error}<p class="small warn" role="status">{draft.capture.error}</p>{/if}
      {#if draft.capture?.note}<p class="small warn" role="status">{draft.capture.note}</p>{/if}
      {#if exactSupported && draft.capture && draft.capture.info.method === "dom"}
        <p class="exact small" class:suggested={draft.capture.suggestExact}>
          {#if draft.capture.suggestExact}<span>Part of this didn't render (a frame, canvas or video).</span>{/if}
          <button type="button" class:primary={draft.capture.suggestExact} disabled={exacting} onclick={() => void useExact()}>Use exact screenshot</button>
        </p>
      {/if}
      {#if (reopening || (domCapture && barModes.length && draft.clip.state === "none")) && !bar}
        <p class="small"><button type="button" class="ghost" onclick={addScreenshot}>{draft.clip.state === "none" ? "Add a screenshot" : "Retake the screenshot"}</button></p>
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
        {#each filters as f (f.id)}
          {#if rawCapture}
            <label><input type="checkbox" aria-label={f.label} checked={filtersOn.includes(f.id)} onchange={e => toggleFilter(f.id, (e.currentTarget as HTMLInputElement).checked)} /> {f.label}</label>
          {/if}
        {/each}
        {#each attachments as a (a.id)}
          <label><input type="checkbox" aria-label={a.label} checked={attachOn.includes(a.id)} onchange={e => { const on = (e.currentTarget as HTMLInputElement).checked; attachOn = on ? [...attachOn, a.id] : attachOn.filter(x => x !== a.id); }} /> {a.label}</label>
        {/each}
      </div>
      {#if outgoing}<Preview sent={outgoing} marked={draft.shapes.length > 0} sentRows={attachRows.filter(r => attachOn.includes(r.id))} filtered={filterNotes} />{/if}
      {#if draft.marksNote}<p class="small warn marks" role="status">{draft.marksNote}</p>{/if}
      {#if draft.error}<p class="error small" role="alert">Not sent: {draft.error}</p>{/if}
      <div class="actions">
        <button type="button" onclick={cancel}>Cancel</button>
        <button type="submit" class="primary" disabled={!canSend(draft)}>{draft.sending ? "Sending…" : "Send"}</button>
      </div>
    </form>
  {:else if inWindow && bar}
    <p class="muted waiting" role="status">Pick what your report is about on the page, or press Esc there.</p>
  {/if}
{/snippet}

<div class="cx" bind:this={cx}>
  {#if bar}
    <ModeBar mode={bar.mode} {outlined} modes={barModes} onMode={choose} onCancel={() => closeBar()} />
  {/if}

  <!-- The notices zone: always there, so a notice never moves the composer or the launcher. -->
  <p class="notice" class:shown={!!notice} role="status">{notice ?? ""}</p>

  {#if draft && !open && !inWindow && dockLayout !== "dock"}
    <div class="scrim" aria-hidden="true" onclick={cancel}></div>
  {/if}
  <div class="dock" data-layout={dockLayout}>
  {#if !inWindow}
    {#if draft && !open && dockLayout === "dialog"}
      <div class="frame" role="dialog" aria-modal="true" aria-labelledby="cx-composer-title" tabindex="-1" onkeydown={trapFocus}>{@render composerSnippet()}</div>
    {:else if draft && !open && dockLayout !== "dock"}
      <div class="frame" role="region" aria-labelledby="cx-composer-title">{@render composerSnippet()}</div>
    {:else}
      {@render composerSnippet()}
    {/if}
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
  .dock { position: fixed; z-index: 2; right: 16px; bottom: 16px; display: flex; flex-direction: column; align-items: flex-end; gap: 8px; max-height: calc(100vh - 32px); pointer-events: none; }
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
  /* The notices zone: fixed at the top right, out of the dock, so a notice never moves anything; hidden while empty. */
  .notice { position: fixed; top: 12px; right: 16px; z-index: 5; pointer-events: none; }
  .notice:not(.shown) { visibility: hidden; padding: 0; border: 0; box-shadow: none; }
  /* Presentations: a dialog in the middle, a drawer on the right, a sheet from the bottom. */
  .scrim { position: fixed; inset: 0; background: rgb(0 0 0 / .32); z-index: 1; }
  .dock[data-layout="dialog"] .frame { position: fixed; left: 50%; top: 50%; transform: translate(-50%, -50%); width: min(560px, calc(100vw - 32px)); max-height: calc(100vh - 48px); display: flex; z-index: 4; }
  .dock[data-layout="drawer"] .frame { position: fixed; top: 0; right: 0; bottom: 0; width: min(440px, 100vw); display: flex; z-index: 4; }
  .dock[data-layout="sheet"] .frame { position: fixed; left: 0; right: 0; bottom: 0; max-height: 80vh; display: flex; z-index: 4; }
  .frame > .composer { width: 100%; max-height: 100%; }
  .dock[data-layout="drawer"] .composer { border-radius: 0; border-width: 0 0 0 1px; border-top: 3px solid var(--you); }
  .dock[data-layout="sheet"] .composer { border-radius: var(--radius) var(--radius) 0 0; }
  .head { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 8px; }
  .head h2 { margin: 0; font: 600 16px/1.3 var(--font); }
  .shot { display: block; max-width: 100%; max-height: 240px; margin-bottom: 8px; border: 1px solid var(--border-strong); border-radius: var(--radius-xs); }
  .waiting { padding: 16px; }
  :global(.cx.window) .composer { width: 100%; max-width: 640px; margin: 0 auto; box-shadow: none; border-radius: 0; min-height: 100vh; }
  @media (prefers-reduced-motion: no-preference) { .dock[data-layout="sheet"] .frame, .dock[data-layout="drawer"] .frame { transition: transform var(--t); } }
  .notice, .filed { flex: none; margin: 0; max-width: min(360px, calc(100vw - 32px)); padding: 8px 12px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius-sm); box-shadow: var(--elev-lg); font-size: 13px; }
  .filed { border-top: 3px solid var(--agent-ink); }
  .filed .actions { margin-top: 6px; }
  @media (max-width: 480px) {
    .composer { position: fixed; left: 0; right: 0; bottom: 0; width: auto; max-height: 70vh; border-width: 3px 0 0; border-radius: var(--radius) var(--radius) 0 0; z-index: 4; }
  }
</style>
