import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { type AnchorResult, INDEX_FILE, type ShellToBridge } from "../../bridge/src/protocol";
import { ApiError, type Artifact, type Version, getArtifact, getToken } from "./api";
import { acceptFromFrame, helloMatches, sendToFrame } from "./bridge-link";
import { CAPTURE_LATE, Composer, type Draft, MAX_CLIP_BYTES, Pins, captureWait, nextDraft, takePick, withClip } from "./comments";
import type { Declared } from "./caps/availability";
import { frameGesture } from "./caps/gesture";
import { CapabilityHost, type CommentsUi } from "./caps/host";
import { type ArtifactEvent, subscribe } from "./events";
import { LOAD_FAILED, OPEN_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, report, scopedNotice } from "./failure";
import { Frame } from "./frame";
import { nav } from "./nav";
import { type Ask, PromptDialog, promptQueue } from "./prompt";
import { artifactOrigin, pageSrc, probeOrigin } from "./origin";
import { parseShellPath, shellPath } from "./route";
import { Sidebar } from "./sidebar";
import { type Thread, type Viewer, addComment, createThread, currentViewer, getViewer, listThreads, onViewer, resolveThread, sendToAgent, upsert } from "./threads";
import { ViewerName } from "./viewer-name";

/** `file` is the page the frame opens on, from the shell URL (`index.html` when it names none). */
type Props = { id: string; pinnedVersion: number | null; file?: string };

/** How long a pick's start stays valid for its pick (longer than the
 * longest clip render, an area's 12 s). */
const PICK_WAIT_MS = 20_000;

/** The notice kind for a thread the daemon kept without its screenshot. */
const CLIP_DROPPED = "Posted without its screenshot";

/** A fragment the frame may report or a link may carry: "" or `#…`, at most 512 characters. */
const validHash = (h: unknown): h is string => typeof h === "string" && (h === "" || h.startsWith("#")) && h.length <= 512;

/** Replaces (or, with `push`, adds) the shell's history entry for `url`;
 * false when the browser refuses (Safari and Firefox throw a SecurityError
 * past their rate limits), leaving the address bar as it was. */
function setUrl(url: string, push = false): boolean {
  try {
    if (push) history.pushState(null, "", url);
    else history.replaceState(history.state, "", url);
    return true;
  } catch {
    return false;
  }
}

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
  // The thread whose card or pin the pointer is over; it, else the selected
  // thread, is the frame's focus (a drawn area is outlined dashed).
  const [hovered, setHovered] = useState<string | null>(null);
  // Posts and sends to the agent in flight (an area compose waits for none).
  const [busy, setBusy] = useState(0);
  const busyRef = useRef(0);
  busyRef.current = busy;
  const whileBusy = <T,>(p: Promise<T>): Promise<T> => { setBusy(n => n + 1); return p.finally(() => setBusy(n => n - 1)); };
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
  const sendRef = useRef(send);
  sendRef.current = send;
  const focusRef = useRef<string | null>(null);
  // Picks whose start arrived with the viewer's gesture in the frame, by pick
  // ID, with when it arrived.
  const startedPicks = useRef(new Map<string, number>());
  // The pick ID of the open composer when a pick made in comment mode opened
  // it: comment mode, off while that composer is open, comes back on when it
  // closes.
  const resumeAfter = useRef<string | null>(null);
  focusRef.current = hovered ?? selected;
  const [ask, setAsk] = useState<Ask | null>(null);
  const prompt = useMemo(() => promptQueue(setAsk), []);
  const hostRef = useRef<CapabilityHost | null>(null);
  // The comment UI as the `comments` capability drives it. Rebuilt every
  // render over refs; the host holds `commentsUi`, which always calls the latest.
  const draftRef = useRef<Draft | null>(null);
  draftRef.current = draft;
  const commentingRef = useRef(false);
  commentingRef.current = commenting;
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selected;
  // What the open composer holds, so a page's open never replaces typed text.
  const composerText = useRef("");
  // A page anchors threads itself (comments.customAnchors): pins come from
  // its placements only, and the frame is not asked to resolve anchors.
  const customLive = useRef(false);
  const uiRef = useRef<CommentsUi | null>(null);
  uiRef.current = {
    openComposer: (d, opts) => {
      const next = nextDraft(draftRef.current, composerText.current, d, opts);
      if (!next) return false;
      setDraft(next);
      return true;
    },
    attachClip: (token, clip, clipError) => setDraft(dr => withClip(dr, token, clip, clipError)),
    upsert: t => changeThreads(ts => upsert(ts, t)),
    remove: tid => { changeThreads(ts => ts.filter(t => t.id !== tid)); setSelected(s => (s === tid ? null : s)); },
    setCustom: live => {
      customLive.current = live;
      if (live) setResolved({});
      else resolveAll();
    },
    place: rects => setResolved(Object.fromEntries(Object.entries(rects).map(([tid, rect]) => [tid, { id: tid, found: true, method: "custom" as const, rect }]))),
    select: tid => { setPanel(true); setSelected(tid); },
    exitMode: () => { if (!composerText.current.trim()) setCommenting(false); },
    dismiss: () => {
      if (draftRef.current) {
        if (composerText.current.trim()) return false;
        setDraft(null);
        return true;
      }
      if (selectedRef.current === null) return false;
      setSelected(null);
      return true;
    },
    enterMode: () => setCommenting(true),
    state: () => ({ mode: commentingRef.current, composing: draftRef.current !== null, threads: threadsRef.current, selected: selectedRef.current, busy: busyRef.current > 0 }),
  };
  const commentsUi = useMemo<CommentsUi>(() => ({
    openComposer: (d, opts) => uiRef.current!.openComposer(d, opts),
    attachClip: (token, clip, clipError) => uiRef.current!.attachClip!(token, clip, clipError),
    upsert: t => uiRef.current!.upsert(t),
    remove: tid => uiRef.current!.remove(tid),
    setCustom: live => uiRef.current!.setCustom(live),
    place: rects => uiRef.current!.place(rects),
    select: tid => uiRef.current!.select(tid),
    exitMode: () => uiRef.current!.exitMode(),
    state: () => uiRef.current!.state(),
    dismiss: () => uiRef.current!.dismiss!(),
    enterMode: () => uiRef.current!.enterMode!(),
  }), []);
  // Thread anchors go to the frame under opaque handles, new for every page
  // that greets, so a page never learns a thread's store ID; the frame's
  // results are mapped back here.
  const anchorIds = useRef({ byHandle: new Map<string, string>(), byThread: new Map<string, string>() });
  const forgetAnchorIds = () => { anchorIds.current = { byHandle: new Map(), byThread: new Map() }; };
  const anchorHandle = (tid: string) => {
    const ids = anchorIds.current;
    let h = ids.byThread.get(tid);
    if (!h) {
      const b = new Uint8Array(12);
      crypto.getRandomValues(b);
      h = `a${Array.from(b, x => x.toString(16).padStart(2, "0")).join("")}`;
      ids.byThread.set(tid, h);
      ids.byHandle.set(h, tid);
    }
    return h;
  };
  /** Tells the frame which thread's drawn area to outline dashed: the
   * hovered, else the selected thread, by the handle the frame knows it by
   * (none when it was not sent to this page). */
  const sendFocus = () => {
    const tid = focusRef.current;
    send({ type: "artifax:focus", id: tid === null ? null : anchorIds.current.byThread.get(tid) ?? null });
  };
  // How many of this view's own page publishes are in flight (the `artifact`
  // handler counts them; one that ends in a reload keeps its count). A page
  // publish by another view that arrives meanwhile is remembered in
  // `deferredPublish` (its version) and applied once the count drops to 0
  // without a reload: a reload to the latest, or the banner when pinned.
  const ownPublish = useRef<{ active: number; settled?(): void }>({ active: 0 });
  const deferredPublish = useRef<number | null>(null);
  ownPublish.current.settled = () => {
    const n = deferredPublish.current;
    deferredPublish.current = null;
    if (n === null) return;
    if (pinnedVersion === null) nav.assign(here(null));
    else if (n > latestKnown.current) { latestKnown.current = n; setNewer(n); }
  };
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
  // The pending animation frame that copies `frameHash` into the address bar.
  const hashFrame = useRef(0);
  useEffect(() => () => { if (hashFrame.current) cancelAnimationFrame(hashFrame.current); }, []);
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
      reload: () => nav.assign(here(null)),
      ownPublish: ownPublish.current,
      page: () => fileRef.current,
      comments: commentsUi,
      files: data.versions.find(v => v.n === shown)?.files,
    })));
  }, [id, shown, origin, data]);
  hostRef.current = host;
  // A replaced host (another artifact, version, or view) and the host at
  // unmount are disposed, so their timers and late results never reach a frame.
  useEffect(() => () => host?.dispose(), [host]);
  const resolveAll = () => {
    if (customLive.current) return;
    send({ type: "artifax:resolve-anchors", requestId: `r${Date.now()}`, anchors: threadsRef.current.filter(t => t.anchor.file === fileRef.current).map(t => ({ id: anchorHandle(t.id), anchor: t.anchor, sameVersion: t.version_n === shown })) });
  };
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
  const saveThread = (p: Promise<Thread>, prefix: string) => { void report(whileBusy(p), prefix, noticeFor(prefix)).then(t => { if (t) changeThreads(ts => upsert(ts, t)); }); };
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
   * shell document was reloaded. When the browser refuses the push, the frame
   * still moves, with an entry of its own (the page's greeting then updates
   * the address bar in place). */
  const openPage = (target: string, hash = "") => {
    const pushed = setUrl(shellPath(id, pinnedVersion, target, shown) + hash, true);
    navigateFrame(target, pushed, hash);
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
    if (t.anchor.file === fileRef.current) {
      // A custom-anchors page brings its own threads into view; the shell
      // never scrolls it.
      if (!hostRef.current?.reveal(t.id)) send({ type: "artifax:scroll-to", anchor: t.anchor, sameVersion: t.version_n === shown });
      return;
    }
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
  useEffect(() => {
    send({ type: "artifax:comment-mode", on: commenting });
    // A pick still in flight when comment mode ends is moot: its start must
    // not stand in the way of the viewer's next pick.
    if (!commenting) startedPicks.current.clear();
  }, [commenting]);
  // The pick's composer closed (posted, cancelled, or dismissed): comment mode
  // comes back on, so the viewer can pick the next target at once. One
  // replaced by another composer, or closed after the viewer pressed Comment
  // or the artifact was deleted, does not.
  useEffect(() => {
    const pick = resumeAfter.current;
    if (pick === null || draft?.pickId === pick) return;
    resumeAfter.current = null;
    if (!draft && !deleted) setCommenting(true);
  }, [draft]);
  useEffect(() => { sendFocus(); }, [hovered, selected, threads]);
  // A screenshot still being taken that never arrives: the composer says so
  // (Post stays disabled until then).
  useEffect(() => {
    const token = draft?.capturing ? draft.clipToken : undefined;
    if (!token) return;
    const timer = setTimeout(() => setDraft(dr => withClip(dr, token, null, CAPTURE_LATE)), captureWait.ms);
    return () => clearTimeout(timer);
  }, [draft?.clipToken, draft?.capturing]);
  useEffect(() => { hostRef.current?.uiChanged(); }, [commenting, draft, selected, threads, file, host, busy]);
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
        startedPicks.current.clear();
        setResolved({});
        forgetAnchorIds();
        if (!helloOk.current) { setCurrentFile(null); break; }
        helloSinceLoad.current = true;
        hostRef.current?.reset();
        setCurrentFile(greeted);
        // The address bar follows the frame to another page. A link the page
        // handed over already pushed its URL; any other navigation (a script,
        // a form) made the frame's own history entry, so the URL is replaced.
        const r = parseShellPath(location.pathname);
        if (r.kind !== "artifact" || r.file !== greeted) setUrl(here(pinnedVersion) + location.hash);
        send({ type: "artifax:welcome", mode: commenting ? "comment" : "view" });
        resolveAll();
        sendFocus();
        const p = pendingScroll.current;
        if (p) {
          // The jump's page greeted: scroll there. Another page greeted: the
          // viewer moved on, so the jump is dropped without a notice.
          clearPending();
          if (p.thread.anchor.file === greeted) send({ type: "artifax:scroll-to", anchor: p.thread.anchor, sameVersion: p.thread.version_n === shown });
        }
        break;
      }
      case "artifax:pick-start":
        // The viewer's pick itself: taken only in comment mode and while the
        // frame holds the viewer's gesture (`frameGesture`). So a page can
        // post a pick of its own only within the user-activation window
        // (about five seconds) after the viewer's latest input to the shell
        // or the frame, while focus is in the frame and no input has reached
        // the shell since focus entered it (the page can move focus into
        // itself, so a click on the shell's Comment button is enough), and
        // while no pick of the bridge's is pending; the composer then shows
        // the pick's quote or area label
        // and screenshot, not where it anchors, and nothing is posted without
        // the viewer. The bridge never has two picks in flight, so a start
        // arriving while another is pending means one was forged: both are
        // refused.
        if (helloOk.current && commentingRef.current && typeof m.pickId === "string" && m.pickId.length <= 64 && frameGesture()) {
          const now = Date.now();
          for (const [pid, at] of startedPicks.current) if (now - at > PICK_WAIT_MS) startedPicks.current.delete(pid);
          if (startedPicks.current.size) startedPicks.current.clear();
          else startedPicks.current.set(m.pickId, now);
        }
        break;
      case "artifax:pick": {
        // A pick counts only after its gesture-checked start (each start
        // once, used up even when the pick is dropped), while the viewer is in
        // comment mode; a clip the daemon would not keep is dropped here, with
        // the reason shown.
        if (!takePick(startedPicks.current, m.pickId, commentingRef.current)) break;
        setCommenting(false);
        resumeAfter.current = m.pickId;
        const png = m.clipPng instanceof ArrayBuffer && m.clipPng.byteLength > 0 ? m.clipPng : null;
        const tooBig = !!png && png.byteLength > MAX_CLIP_BYTES;
        setDraft({ pickId: m.pickId, anchor: m.anchor, version: m.version, clip: png && !tooBig ? new Blob([png], { type: "image/png" }) : null, clipError: tooBig ? "the screenshot was too large to keep" : m.clipError });
        break;
      }
      case "artifax:anchors": {
        if (customLive.current) break;
        const byHandle = anchorIds.current.byHandle;
        setResolved(prev => {
          const next = m.requestId ? {} as Record<string, AnchorResult> : { ...prev };
          for (const r of m.results) {
            const tid = byHandle.get(r.id);
            if (tid) next[tid] = { ...r, id: tid };
          }
          return next;
        });
        break;
      }
      case "artifax:cancel": setCommenting(false); break;
      case "artifax:hash":
        // The page's fragment moved (a link, a script): the address bar
        // follows in place, once per animation frame with the latest
        // fragment; the frame's own history entry carries the move.
        if (!helloOk.current || !validHash(m.hash)) break;
        frameHash.current = m.hash;
        if (!hashFrame.current) {
          hashFrame.current = requestAnimationFrame(() => {
            hashFrame.current = 0;
            if (location.hash !== frameHash.current) setUrl(location.pathname + location.search + frameHash.current);
          });
        }
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
    // Whether the pointer is over the content frame (the shell sees a
    // mouseover on the iframe element as it enters, and on another element
    // as it leaves).
    let overFrame = false;
    const onOver = (e: MouseEvent) => { overFrame = e.target === frameRef.current; };
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.localName === "input" || t.localName === "textarea" || t.isContentEditable);
      // Escape with the pointer over the frame in comment mode goes to the
      // page, which drops a drag in progress or else answers artifax:cancel
      // (ending comment mode); anywhere else it ends comment mode here.
      if (e.key === "Escape" && !(commentingRef.current && overFrame && !typing)) {
        if (e.type === "keydown") setCommenting(false);
        return;
      }
      // Option widening works with focus in the shell: while comment mode is
      // on and the pointer is over the frame, Option and, with it held, Up
      // and Down are forwarded to the page (not from a text field).
      if (!commentingRef.current || !overFrame || typing) return;
      const down = e.type === "keydown";
      if (e.key === "Alt" || e.key === "Escape" || ((e.key === "ArrowUp" || e.key === "ArrowDown") && e.altKey)) {
        sendRef.current({ type: "artifax:key", key: e.key, down });
        if (e.key === "ArrowUp" || e.key === "ArrowDown") e.preventDefault();
      }
    };
    addEventListener("mouseover", onOver);
    addEventListener("keydown", onKey);
    addEventListener("keyup", onKey);
    return () => { removeEventListener("message", onMessage); removeEventListener("mouseover", onOver); removeEventListener("keydown", onKey); removeEventListener("keyup", onKey); };
  }, []);

  const onEventRef = useRef<(e: ArtifactEvent) => void>(() => {});
  onEventRef.current = e => {
    hostRef.current?.onEvent(e);
    if (e.type === "version" && e.by_page && e.n > shown) {
      // The page republished itself (artifact.publish): every unpinned view
      // follows at once, on the page it shows (the new version carries every
      // file forward); a pinned view gets the banner. The publishing view
      // reloads itself after its call result is posted; while one of its own
      // publishes is in flight, another view's publish waits for it to settle.
      if (ownPublish.current.active > 0) { deferredPublish.current = Math.max(deferredPublish.current ?? 0, e.n); return; }
      if (pinnedVersion === null) {
        latestKnown.current = Math.max(latestKnown.current, e.n);
        nav.assign(here(null));
        return;
      }
    }
    if (e.type === "version" && e.n > latestKnown.current) { latestKnown.current = e.n; setNewer(e.n); }
    if (e.type === "artifact_deleted") setDeleted(true);
    if (e.type === "thread") changeThreads(ts => upsert(ts, e.thread));
    if (e.type === "thread_deleted") { changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); setSelected(s => (s === e.thread_id ? null : s)); }
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
        <button aria-pressed={commenting} class={commenting ? "primary" : ""} disabled={deleted} onClick={() => { resumeAfter.current = null; setCommenting(c => !c); }}>Comment</button>
        <button aria-pressed={panel} onClick={() => setPanel(v => !v)}>Threads ({threads.filter(t => t.status === "open").length})</button>
        {!narrow && <ViewerName setNotice={setNotice} onViewer={setMe} />}
        <select value={shown} disabled={deleted} onChange={e => { const n = Number((e.target as HTMLSelectElement).value); nav.assign(here(n === latest ? null : n)); }}>
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
          {!deleted && !missing && <Pins threads={threads} resolved={resolved} file={file} onSelect={t => { setPanel(true); scrollTo(t); }} onHover={t => setHovered(t?.id ?? null)} />}
          {draft && <Composer key={draft.pickId} draft={draft} onText={v => { composerText.current = v; }} onCancel={() => setDraft(null)} onSubmit={async body => {
            try {
              const { thread, clip_error: clipError } = await whileBusy(createThread(id, { anchor: draft.anchor, body, version: draft.version, clip: draft.clip }));
              noticeFor(POST_FAILED)(null);
              noticeFor(CLIP_DROPPED)(clipError ? `${CLIP_DROPPED}: ${clipError}` : null);
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
            <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => nav.assign(here(null))}>Reload</button></div>
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
          onHover={t => setHovered(t?.id ?? null)}
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
