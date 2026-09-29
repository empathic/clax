import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type { AnchorResult, ShellToBridge } from "../../bridge/src/protocol";
import { ApiError, type Artifact, type Version, getArtifact } from "./api";
import { acceptFromFrame, sendToFrame } from "./bridge-link";
import { Composer, type Draft, Pins } from "./comments";
import { subscribe } from "./events";
import { LOAD_FAILED, NAME_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, report } from "./failure";
import { Frame } from "./frame";
import { artifactOrigin, contentSrc, probeOrigin } from "./origin";
import { Sidebar } from "./sidebar";
import { type Thread, addComment, createThread, listThreads, resolveThread, sendToAgent, upsert } from "./threads";
import { ViewerName } from "./viewer-name";

type Props = { id: string; pinnedVersion: number | null };

export default function ArtifactView({ id, pinnedVersion }: Props) {
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
  const [now, setNow] = useState(() => new Date());
  const [notice, setNotice] = useState<string | null>(null);
  // A success clears only a notice its own kind of call raised, so the viewer
  // lookup finishing after a failed thread load cannot hide that failure.
  const noticeFor = (prefix: string) => (text: string | null) =>
    setNotice(cur => (text !== null ? text : cur?.startsWith(`${prefix}:`) ? null : cur));
  const threadsRef = useRef<Thread[]>([]);
  threadsRef.current = threads;

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
  const resolveAll = () => send({ type: "artifax:resolve-anchors", requestId: `r${Date.now()}`, anchors: threadsRef.current.map(t => ({ id: t.id, anchor: t.anchor })) });
  const loadThreads = () => { void report(listThreads(id), LOAD_FAILED, noticeFor(LOAD_FAILED)).then(ts => { if (ts) setThreads(ts); }); };
  const saveThread = (p: Promise<Thread>, prefix: string) => { void report(p, prefix, noticeFor(prefix)).then(t => { if (t) setThreads(ts => upsert(ts, t)); }); };
  const scrollTo = (t: Thread) => { setSelected(t.id); send({ type: "artifax:scroll-to", anchor: t.anchor }); };

  useEffect(loadThreads, [id]);
  useEffect(() => { resolveAll(); }, [threads.map(t => t.id).join(","), shown, origin]);
  useEffect(() => { send({ type: "artifax:comment-mode", on: commenting }); }, [commenting]);
  useEffect(() => {
    if (!threads.some(t => t.status === "open" && t.sent_to_agent && t.feedback_state && t.feedback_state.state !== "acknowledged")) return;
    const timer = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(timer);
  }, [threads]);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia("(max-width: 480px)");
    const onChange = () => setNarrow(mq.matches);
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, []);
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const m = acceptFromFrame(e, frameWin(), origin ?? null);
      if (!m) return;
      switch (m.type) {
        case "artifax:hello": send({ type: "artifax:welcome", mode: commenting ? "comment" : "view" }); resolveAll(); break;
        case "artifax:pick": setCommenting(false); setDraft({ anchor: m.anchor, version: m.version, clip: m.clipPng ? new Blob([m.clipPng], { type: "image/png" }) : null, clipError: m.clipError }); break;
        case "artifax:anchors": setResolved(prev => { const next = m.requestId ? {} as Record<string, AnchorResult> : { ...prev }; for (const r of m.results) next[r.id] = r; return next; }); break;
        case "artifax:cancel": setCommenting(false); break;
        case "artifax:hover": break;
      }
    };
    addEventListener("message", onMessage);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setCommenting(false); };
    addEventListener("keydown", onKey);
    return () => { removeEventListener("message", onMessage); removeEventListener("keydown", onKey); };
  }, [origin, commenting]);

  useEffect(() => subscribe(id, e => {
    if (e.type === "version" && e.n > latestKnown.current) { latestKnown.current = e.n; setNewer(e.n); }
    if (e.type === "artifact_deleted") setDeleted(true);
    if (e.type === "thread") setThreads(ts => upsert(ts, e.thread));
    if (e.type === "feedback_state") setThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
    if (e.type === "resync") {
      loadThreads();
      getArtifact(id).then(d => {
        const n = d.artifact.current_version;
        if (n > latestKnown.current) { latestKnown.current = n; setNewer(n); }
      }, err => { if (err instanceof ApiError && err.status === 404) setDeleted(true); });
    }
  }), [id]);

  if (error) return <Shell title="Artifax"><p class="empty">{error}</p></Shell>;
  if (!data || origin === undefined) return <Shell title="Artifax"><p class="empty muted">Loading…</p></Shell>;
  const { artifact, versions } = data;
  const latest = artifact.current_version;
  const raw = contentSrc(id, shown, origin);

  return (
    <Shell title={artifact.title} right={
      <>
        <button aria-pressed={commenting} class={commenting ? "primary" : ""} disabled={deleted} onClick={() => setCommenting(c => !c)}>Comment</button>
        <button aria-pressed={panel} onClick={() => setPanel(v => !v)}>Threads ({threads.filter(t => t.status === "open").length})</button>
        {!narrow && <ViewerName setNotice={noticeFor(NAME_FAILED)} />}
        <select value={shown} disabled={deleted} onChange={e => { const n = Number((e.target as HTMLSelectElement).value); location.assign(n === latest ? `/a/${id}` : `/a/${id}/v/${n}`); }}>
          {versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
        </select>
        {deleted
          ? <span class="hide-sm muted">open raw</span>
          : <a class="hide-sm" href={raw} target="_blank" rel="noopener">open raw</a>}
        {navigator.clipboard && (
          <button disabled={deleted} onClick={() => { navigator.clipboard.writeText(location.origin + `/a/${id}`).catch(() => {}); }}>copy link</button>
        )}
      </>
    }>
      <div class={`viewer${panel ? " with-sidebar" : ""}`}>
        <div class="stage">
          {deleted ? <p class="empty">This artifact was deleted.</p> : <Frame id={id} n={shown} origin={origin} frameRef={frameRef} />}
          {!deleted && <Pins threads={threads} resolved={resolved} onSelect={t => { setPanel(true); scrollTo(t); }} />}
          {draft && <Composer draft={draft} onCancel={() => setDraft(null)} onSubmit={async body => {
            try {
              const { thread } = await createThread(id, { anchor: draft.anchor, body, version: draft.version, clip: draft.clip });
              noticeFor(POST_FAILED)(null);
              setThreads(ts => upsert(ts, thread));
              setSelected(thread.id);
              setDraft(null);
              setPanel(true);
            } catch (e) {
              void report(Promise.reject(e), POST_FAILED, noticeFor(POST_FAILED));
              throw e;
            }
          }} />}
          {newer && !deleted && (
            <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => location.assign(`/a/${id}`)}>Reload</button></div>
          )}
          {shown < latest && !newer && !deleted && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={`/a/${id}`}>latest</a></div>}
          {notice && (
            <div class="banner notice" role="alert"><span>{notice}</span><button onClick={() => setNotice(null)}>Dismiss</button></div>
          )}
        </div>
        {panel && <Sidebar threads={threads} resolved={resolved} now={now} selected={selected}
          header={narrow ? <ViewerName setNotice={noticeFor(NAME_FAILED)} /> : undefined}
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
