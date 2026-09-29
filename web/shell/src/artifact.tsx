import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { type AnchorResult, INDEX_FILE, type ShellToBridge } from "../../bridge/src/protocol";
import { ApiError, type Artifact, type Version, getArtifact, getToken } from "./api";
import { acceptFromFrame, helloMatches, sendToFrame } from "./bridge-link";
import { Composer, type Draft, Pins } from "./comments";
import type { Declared } from "./caps/availability";
import { CapabilityHost } from "./caps/host";
import { type ArtifactEvent, subscribe } from "./events";
import { LOAD_FAILED, OPEN_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, report, scopedNotice } from "./failure";
import { Frame } from "./frame";
import { type Ask, PromptDialog, promptQueue } from "./prompt";
import { artifactOrigin, pageSrc, probeOrigin } from "./origin";
import { parseShellPath, shellPath } from "./route";
import { Sidebar } from "./sidebar";
import { type Thread, type Viewer, addComment, createThread, currentViewer, getViewer, listThreads, onViewer, resolveThread, sendToAgent, upsert } from "./threads";
import { ViewerName } from "./viewer-name";

/** `file` is the page the frame opens on, from the shell URL (`index.html` when it names none). */
type Props = { id: string; pinnedVersion: number | null; file?: string };

/** A fragment the frame may report or a link may carry: "" or `#…`, at most 512 characters. */
const validHash = (h: unknown): h is string => typeof h === "string" && (h === "" || h.startsWith("#")) && h.length <= 512;

/** How long a page the shell sent the frame to may take to greet before the
 * jump is given up (settable for tests). */
export const pageWait = { ms: 5000 };

export default function ArtifactView({ id, pinnedVersion, file: startFile = INDEX_FILE }: Props) {
  const [data, setData] = useState<{ artifact: Artifact; versions: Version[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [origin, setOrigin] = useState<string | null | undefined>(undefined);
  const [newer, setNewer] = useState<number | null>(null);
  const [deleted, setDeleted] = useState(false);

  const latestKnown = useRef(0);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const [commenting, setCommenting] = useState(false);
  const [panel, setPanel] = useState(() => typeof matchMedia === "function" && matchMedia("(min-width: 900px)").matches);
  const [narrow, setNarrow] = useState(() => typeof matchMedia === "function" && matchMedia("(max-width: 480px)").matches);
  const [threads, setThreads] = useState<Thread[]>([]);
  const [resolved, setResolved] = useState<Record<string, AnchorResult>>({});
  const [draft, setDraft] = useState<Draft | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [me, setMe] = useState<Viewer | null>(null);
  // A success clears only a notice its own kind of call raised, so the viewer
  // lookup finishing after a failed thread load cannot hide that failure.
  const noticeFor = (prefix: string) => scopedNotice(setNotice, prefix);
  const threadsRef = useRef<Thread[]>([]);
  threadsRef.current = threads;
  // Thread changes (events and this shell's own writes) since the latest
  // `listThreads` request, kept while it is in flight: its answer may predate
  // them, so they are replayed on top of it. Only the latest request's answer
  // is applied; once it answers or fails, nothing more is kept.
  const threadLoad = useRef<{ n: number; since: ((ts: Thread[]) => Thread[])[] | null }>({ n: 0, since: null });
  const changeThreads = (f: (ts: Thread[]) => Thread[]) => { threadLoad.current.since?.push(f); setThreads(f); };

  useEffect(() => {
    getArtifact(id).then(d => {
      latestKnown.current = Math.max(latestKnown.current, d.artifact.current_version);
      setNewer(prev => (prev !== null && prev <= d.artifact.current_version ? null : prev));
      setData(d);
    }, e => setError(e instanceof ApiError && e.status === 404 ? "Artifact not found" : String(e)));
  }, [id]);
  useEffect(() => {
    const o = artifactOrigin(id);
    if (!o) { setOrigin(null); return; }
    probeOrigin(o).then(ok => setOrigin(ok ? o : null));
  }, [id]);

  const shown = pinnedVersion ?? data?.artifact.current_version ?? 0;

  const frameWin = () => frameRef.current?.contentWindow ?? null;
  const send = (m: ShellToBridge) => sendToFrame(frameWin(), origin ?? null, m);
  const [ask, setAsk] = useState<Ask | null>(null);
  const prompt = useMemo(() => promptQueue(setAsk), []);
  const hostRef = useRef<CapabilityHost | null>(null);
  // Whether the frame's latest hello named the shown artifact and version: only
  // then are its capability requests answered and events pushed to it, so a
  // document the frame navigated to gets nothing. A frame load with no
  // matching hello since the previous load closes the gate as well, and a
  // later hello reopens it: a wrapped page's hello may arrive before or after
  // its load event (after it for some sandboxed loads), so this never shuts
  // out a wrapped page, and it shuts out a document without the bridge
  // whenever the page before it greeted before its own load.
  const helloOk = useRef(false);
  const helloSinceLoad = useRef(false);
  // Reset while rendering, not in an effect: the frame for a new artifact,
  // version, or origin is inserted by this render, and its hello can arrive
  // before this render's effects run.
  const gateFor = useRef("");
  if (gateFor.current !== `${id}/${shown}/${origin}`) {
    gateFor.current = `${id}/${shown}/${origin}`;
    helloOk.current = false;
    helloSinceLoad.current = false;
  }
  // The published file of the page in the frame, from its latest matching
  // hello (the URL's file until then): pins, anchor resolution, and scroll-to
  // apply to its threads only, and the shell URL names it. Null while the
  // frame shows a document that did not greet: no pins are drawn over it.
  const [file, setFile] = useState<string | null>(startFile);
  const fileRef = useRef<string | null>(startFile);
  const setCurrentFile = (f: string | null) => { fileRef.current = f; setFile(f); };
  const onFrameLoad = () => {
    if (!helloSinceLoad.current) {
      helloOk.current = false;
      setCurrentFile(null);
      setResolved({});
    }
    helloSinceLoad.current = false;
  };
  // Whether the shown version holds `f` (the index always; any file while the
  // versions are unknown).
  const versionRef = useRef<Version | undefined>(undefined);
  versionRef.current = data?.versions.find(v => v.n === shown);
  const holds = (f: string) => f === INDEX_FILE || !versionRef.current || Object.hasOwn(versionRef.current.files, f);
  /** The shell URL of the current page in `version` (null: the latest). */
  const here = (version: number | null) => {
    const r = parseShellPath(location.pathname);
    return shellPath(id, version, fileRef.current ?? (r.kind === "artifact" ? r.file : INDEX_FILE), shown);
  };
  // A thread on another page the viewer opened: the frame was sent to that
  // page, and it is scrolled to once that page greets, or given up after
  // `pageWait.ms` with a notice.
  const pendingScroll = useRef<{ thread: Thread; timer: ReturnType<typeof setTimeout> } | null>(null);
  // The URL fragment the frame opens at, and the frame's latest known fragment.
  const [startHash] = useState(() => location.hash);
  const frameHash = useRef(startHash);
  const clearPending = () => { if (pendingScroll.current) clearTimeout(pendingScroll.current.timer); pendingScroll.current = null; };
  useEffect(() => clearPending, []);
  // Made while rendering, not in an effect, so it exists before the frame it
  // serves is inserted: a hello and a capability request can arrive before
  // that render's effects run.
  const host = useMemo(() => {
    if (!data || origin === undefined) return null;
    return new CapabilityHost(getToken().then(token => ({
      aid: id,
      version: shown,
      pinned: pinnedVersion !== null,
      token,
      viewer: currentViewer,
      declared: (data.artifact.capabilities ?? {}) as Declared,
      prompt,
      post: m => { if (helloOk.current) send(m); },
      reload: () => location.assign(here(null)),
    })));
  }, [id, shown, origin, data]);
  hostRef.current = host;
  const resolveAll = () => send({ type: "artifax:resolve-anchors", requestId: `r${Date.now()}`, anchors: threadsRef.current.filter(t => t.anchor.file === fileRef.current).map(t => ({ id: t.id, anchor: t.anchor })) });
  const loadThreads = () => {
    const load: { n: number; since: ((ts: Thread[]) => Thread[])[] | null } = { n: threadLoad.current.n + 1, since: [] };
    threadLoad.current = load;
    void report(listThreads(id), LOAD_FAILED, noticeFor(LOAD_FAILED)).then(ts => {
      if (threadLoad.current !== load) return;
      const since = load.since ?? [];
      load.since = null;
      if (ts) setThreads(since.reduce((acc, f) => f(acc), ts));
    });
  };
  const saveThread = (p: Promise<Thread>, prefix: string) => { void report(p, prefix, noticeFor(prefix)).then(t => { if (t) changeThreads(ts => upsert(ts, t)); }); };
  /** Sends the frame to the page published at `target` (at `hash`, a fragment
   * with its `#`); `replace` keeps the frame's history entry (the shell URL
   * made or moved it). The outgoing document is done: the gate closes and
   * its page and pins are forgotten until the next page greets. */
  const navigateFrame = (target: string, replace: boolean, hash = "") => {
    const frame = frameRef.current;
    if (!frame) return;
    frameHash.current = hash;
    helloOk.current = false;
    setCurrentFile(null);
    setResolved({});
    const url = pageSrc(id, shown, origin ?? null, target) + hash;
    if (replace && frame.contentWindow) {
      try { frame.contentWindow.location.replace(url); return; } catch { /* fall back to src */ }
    }
    frame.src = url;
  };
  /** Moves the frame, showing `page`, to the fragment `hash` in place (a
   * fragment navigation keeps the document, so the gate stays open). No
   * fragment is `#`, since dropping it would reload the page. */
  const moveFragment = (page: string, hash: string) => {
    const win = frameRef.current?.contentWindow;
    frameHash.current = hash;
    try { win?.location.replace(pageSrc(id, shown, origin ?? null, page) + (hash || "#")); } catch { /* the frame is gone */ }
  };
  /** Shows `target`, an HTML page of this version, as one history entry: the
   * shell URL is pushed and the frame is moved without an entry of its own, so
   * back and forward (through `popstate`) move between pages, also after the
   * shell document was reloaded. */
  const openPage = (target: string, hash = "") => {
    history.pushState(null, "", shellPath(id, pinnedVersion, target, shown) + hash);
    navigateFrame(target, true, hash);
  };
  /** Whether `f` is an HTML page of the shown version. */
  const isPage = (f: string) => {
    const v = versionRef.current;
    if (f === INDEX_FILE || !v) return true;
    return Object.hasOwn(v.files, f) && v.files[f].content_type.split(";")[0].trim().toLowerCase() === "text/html";
  };
  const scrollTo = (t: Thread) => {
    setSelected(t.id);
    clearPending();
    if (t.anchor.file === fileRef.current) { send({ type: "artifax:scroll-to", anchor: t.anchor }); return; }
    // A thread on a page this version does not hold is detached: nothing to open.
    if (!holds(t.anchor.file)) return;
    const timer = setTimeout(() => {
      if (pendingScroll.current?.thread !== t) return;
      pendingScroll.current = null;
      noticeFor(OPEN_FAILED)(`${OPEN_FAILED} ${t.anchor.file}: the page did not load`);
    }, pageWait.ms);
    pendingScroll.current = { thread: t, timer };
    openPage(t.anchor.file);
  };
  // Back and forward move the shell URL between pages; the frame follows when
  // it shows another page.
  useEffect(() => {
    const onPop = () => {
      clearPending();
      const r = parseShellPath(location.pathname);
      if (r.kind !== "artifact" || r.id !== id) return;
      if (r.file !== fileRef.current) navigateFrame(r.file, true, location.hash);
      else if (location.hash !== frameHash.current) moveFragment(r.file, location.hash);
    };
    addEventListener("popstate", onPop);
    return () => removeEventListener("popstate", onPop);
  }, [id, shown, origin]);

  useEffect(loadThreads, [id]);
  useEffect(() => { resolveAll(); }, [threads.map(t => t.id).join(","), shown, origin]);
  useEffect(() => { send({ type: "artifax:comment-mode", on: commenting }); }, [commenting]);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia("(max-width: 480px)");
    const onChange = () => setNarrow(mq.matches);
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, []);
  // The handler is rebuilt on every render and the listener always calls the
  // latest one, so a hello that arrives before the effects of the render that
  // inserted the frame have run is judged against the shown artifact and version.
  const onMessageRef = useRef<(e: MessageEvent) => void>(() => {});
  onMessageRef.current = (e: MessageEvent) => {
    const m = acceptFromFrame(e, frameWin(), origin ?? null);
    if (!m) return;
    switch (m.type) {
      case "artifax:hello": {
        // A stale or foreign document in the frame, or one naming a page
        // this version does not hold, gets no welcome, no anchors, and no pins.
        const greeted = typeof m.file === "string" && m.file ? m.file : INDEX_FILE;
        helloOk.current = helloMatches(m, id, shown) && holds(greeted);
        setResolved({});
        if (!helloOk.current) { setCurrentFile(null); break; }
        helloSinceLoad.current = true;
        hostRef.current?.reset();
        setCurrentFile(greeted);
        // The address bar follows the frame to another page. A link the page
        // handed over already pushed its URL; any other navigation (a script,
        // a form) made the frame's own history entry, so the URL is replaced.
        const r = parseShellPath(location.pathname);
        if (r.kind !== "artifact" || r.file !== greeted) history.replaceState(history.state, "", here(pinnedVersion) + location.hash);
        send({ type: "artifax:welcome", mode: commenting ? "comment" : "view" });
        resolveAll();
        const p = pendingScroll.current;
        if (p) {
          // The jump's page greeted: scroll there. Another page greeted: the
          // viewer moved on, so the jump is dropped without a notice.
          clearPending();
          if (p.thread.anchor.file === greeted) send({ type: "artifax:scroll-to", anchor: p.thread.anchor });
        }
        break;
      }
      case "artifax:pick": setCommenting(false); setDraft({ pickId: m.pickId, anchor: m.anchor, version: m.version, clip: m.clipPng ? new Blob([m.clipPng], { type: "image/png" }) : null, clipError: m.clipError }); break;
      case "artifax:anchors": setResolved(prev => { const next = m.requestId ? {} as Record<string, AnchorResult> : { ...prev }; for (const r of m.results) next[r.id] = r; return next; }); break;
      case "artifax:cancel": setCommenting(false); break;
      case "artifax:hash":
        // The page's fragment moved (a link, a script): the address bar
        // follows in place; the frame's own history entry carries the move.
        if (!helloOk.current || !validHash(m.hash)) break;
        frameHash.current = m.hash;
        if (location.hash !== m.hash) history.replaceState(history.state, "", location.pathname + location.search + m.hash);
        break;
      case "artifax:navigate": {
        // A link the page handed over, from a document that greeted and is
        // not already leaving (one entry per greeting page). An HTML page of
        // this version is one history entry; another file of the version
        // loads in the frame as a plain link would; anything else is ignored.
        if (!helloOk.current || typeof m.file !== "string" || !holds(m.file)) break;
        const hash = validHash(m.hash) ? m.hash : "";
        clearPending();
        if (isPage(m.file)) openPage(m.file, hash);
        else navigateFrame(m.file, false, hash);
        break;
      }
      case "artifax:hover": break;
      case "artifax:use": case "artifax:call": if (helloOk.current) void hostRef.current?.handle(m); break;
    }
  };
  useEffect(() => {
    const onMessage = (e: MessageEvent) => onMessageRef.current(e);
    addEventListener("message", onMessage);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setCommenting(false); };
    addEventListener("keydown", onKey);
    return () => { removeEventListener("message", onMessage); removeEventListener("keydown", onKey); };
  }, []);

  const onEventRef = useRef<(e: ArtifactEvent) => void>(() => {});
  onEventRef.current = e => {
    hostRef.current?.onEvent(e);
    if (e.type === "version" && e.n > latestKnown.current) { latestKnown.current = e.n; setNewer(e.n); }
    if (e.type === "artifact_deleted") setDeleted(true);
    if (e.type === "thread") changeThreads(ts => upsert(ts, e.thread));
    if (e.type === "feedback_state") changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
    // A (re)connect may follow a daemon restart that dropped events without a
    // resync; reload like a resync. The first one also covers anything
    // published between the initial load and the stream opening.
    if (e.type === "resync" || e.type === "ready") {
      loadThreads();
      getArtifact(id).then(d => {
        const n = d.artifact.current_version;
        if (n > latestKnown.current) { latestKnown.current = n; setNewer(n); }
      }, err => { if (err instanceof ApiError && err.status === 404) setDeleted(true); });
    }
  };
  useEffect(() => {
    let live = true;
    let stop: (() => void) | null = null;
    const open = async (resync: boolean) => {
      // The owner shell passes its token (null on a LAN view) so the daemon
      // counts its stream as the owner shell's.
      const token = await getToken();
      if (!live) return;
      stop?.();
      stop = subscribe(id, e => onEventRef.current(e), token);
      // Events between the old and the new stream are lost: refetch as on a resync.
      if (resync) onEventRef.current({ type: "resync", dropped: 0 });
    };
    // The daemon reads the viewer cookie when the stream opens (its level for
    // `doc` events is fixed then), so open it once the lookup has set the
    // cookie, and reopen when a later lookup or a rename changes the viewer.
    const first = () => { if (live && !stop) void open(false); };
    void getViewer().then(first, first);
    const off = onViewer(() => { if (live && stop) void open(true); });
    return () => { live = false; off(); stop?.(); };
  }, [id]);

  if (error) return <Shell title="Artifax"><p class="empty">{error}</p></Shell>;
  if (!data || origin === undefined) return <Shell title="Artifax"><p class="empty muted">Loading…</p></Shell>;
  const { artifact, versions } = data;
  const latest = artifact.current_version;
  const urlRoute = parseShellPath(location.pathname);
  const raw = pageSrc(id, shown, origin, file ?? (urlRoute.kind === "artifact" ? urlRoute.file : INDEX_FILE));
  const version = versions.find(v => v.n === shown);
  // A page the URL names that the shown version does not hold (the index is
  // always there) gets a message instead of the daemon's 404 in the frame.
  const missing = startFile !== INDEX_FILE && version !== undefined && !Object.hasOwn(version.files, startFile) ? startFile : null;

  return (
    <Shell title={artifact.title} right={
      <>
        <button aria-pressed={commenting} class={commenting ? "primary" : ""} disabled={deleted} onClick={() => setCommenting(c => !c)}>Comment</button>
        <button aria-pressed={panel} onClick={() => setPanel(v => !v)}>Threads ({threads.filter(t => t.status === "open").length})</button>
        {!narrow && <ViewerName setNotice={setNotice} onViewer={setMe} />}
        <select value={shown} disabled={deleted} onChange={e => { const n = Number((e.target as HTMLSelectElement).value); location.assign(here(n === latest ? null : n)); }}>
          {versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
        </select>
        {deleted
          ? <span class="hide-sm muted">open raw</span>
          : <a class="hide-sm" href={raw} target="_blank" rel="noopener">open raw</a>}
        {navigator.clipboard && (
          <button disabled={deleted} onClick={() => { navigator.clipboard.writeText(location.origin + here(pinnedVersion) + location.hash).catch(() => {}); }}>copy link</button>
        )}
      </>
    }>
      <div class={`viewer${panel ? " with-sidebar" : ""}`}>
        <div class="stage">
          {deleted
            ? <p class="empty">This artifact was deleted.</p>
            : missing
              ? <p class="empty">v{shown} has no page {missing}. <a href={shellPath(id, pinnedVersion, INDEX_FILE)}>Open the index</a></p>
              : <Frame id={id} n={shown} origin={origin} file={startFile} hash={startHash} frameRef={frameRef} onLoad={onFrameLoad} />}
          {!deleted && !missing && <Pins threads={threads} resolved={resolved} file={file} onSelect={t => { setPanel(true); scrollTo(t); }} />}
          {draft && <Composer key={draft.pickId} draft={draft} onCancel={() => setDraft(null)} onSubmit={async body => {
            try {
              const { thread } = await createThread(id, { anchor: draft.anchor, body, version: draft.version, clip: draft.clip });
              noticeFor(POST_FAILED)(null);
              changeThreads(ts => upsert(ts, thread));
              setSelected(thread.id);
              setDraft(null);
              setPanel(true);
            } catch (e) {
              void report(Promise.reject(e), POST_FAILED, noticeFor(POST_FAILED));
              throw e;
            }
          }} />}
          {newer && !deleted && (
            <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => location.assign(here(null))}>Reload</button></div>
          )}
          {shown < latest && !newer && !deleted && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={here(null)}>latest</a></div>}
          {notice && (
            <div class="banner notice" role="alert"><span>{notice}</span><button onClick={() => setNotice(null)}>Dismiss</button></div>
          )}
          {ask && <PromptDialog ask={ask} />}
        </div>
        {panel && <Sidebar threads={threads} resolved={resolved} selected={selected} file={file} holds={holds}
          me={me} header={narrow ? <ViewerName setNotice={setNotice} onViewer={setMe} /> : undefined}
          onSelect={scrollTo}
          onSend={t => saveThread(sendToAgent(id, t.id), SEND_FAILED)}
          onResolve={t => saveThread(resolveThread(id, t.id), RESOLVE_FAILED)}
          onReply={(t, body) => saveThread(addComment(id, t.id, body), POST_FAILED)} />}
      </div>
    </Shell>
  );
}

function Shell({ title, right, children }: { title: string; right?: ComponentChildren; children: ComponentChildren }) {
  return (
    <div class="page">
      <header class="topbar">
        <a href="/" title="Gallery">←</a>
        <h1>{title}</h1>
        {right}
      </header>
      {children}
    </div>
  );
}
