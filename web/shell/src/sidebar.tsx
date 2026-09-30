import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { type AnchorResult, INDEX_FILE } from "../../bridge/src/protocol";
import { anchorLabel, resolvedByLabel, type Thread, type Viewer } from "./threads";
import { isSubmitKey } from "./comments";
import { hasElapsedLabel, waitingLabel } from "./waiting";

type Props = {
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  /** Fixed clock for tests; without it the sidebar ticks each second while a label shows elapsed time. */
  now?: Date;
  selected: string | null;
  onSelect(t: Thread): void;
  onSend(t: Thread): void;
  onResolve(t: Thread): void;
  onReply(t: Thread, body: string): void;
  /** Hears the thread whose card the pointer is over, and null when it leaves. */
  onHover?(t: Thread | null): void;
  /** This viewer, to show its own name on threads it resolved. */
  me?: Viewer | null;
  /** Rendered above the sections (the "Your name" field on narrow screens). */
  header?: ComponentChildren;
  /** The page the frame shows (the index by default; null when the frame shows
   * a document that did not greet); threads on other pages are labelled with theirs. */
  file?: string | null;
  /** Whether the shown version holds a page; a thread on a page it lacks is detached. */
  holds?: (file: string) => boolean;
};

/** Open threads: those on the page shown and found (numbered like the pins),
 * then those on other pages of the version (labelled "on <file>"); open
 * threads on the page shown and not found, or on a page the version does not
 * hold (Detached); then resolved threads. */
export function Sidebar(p: Props) {
  const [tick, setTick] = useState(() => new Date());
  const ticking = p.now === undefined && p.threads.some(t => t.status === "open" && t.sent_to_agent && hasElapsedLabel(t.feedback_state));
  useEffect(() => {
    if (!ticking) return;
    setTick(new Date());
    const timer = setInterval(() => setTick(new Date()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  const now = p.now ?? tick;
  const file = p.file === undefined ? INDEX_FILE : p.file;
  const holds = p.holds ?? (() => true);
  const open = p.threads.filter(t => t.status === "open");
  const here = open.filter(t => t.anchor.file === file);
  const gone = open.filter(t => !holds(t.anchor.file));
  const detached = [...here.filter(t => p.resolved[t.id] && !p.resolved[t.id].found), ...gone.filter(t => !here.includes(t))];
  const attached = here.filter(t => !detached.includes(t));
  const elsewhere = open.filter(t => t.anchor.file !== file && !gone.includes(t));
  const done = p.threads.filter(t => t.status === "resolved");
  const numbers = new Map(attached.map((t, i) => [t.id, i + 1]));
  const section = (cls: string, title: string, list: Thread[]) => (
    <section class={cls}>
      <h2>{title} <span class="muted">{list.length}</span></h2>
      {list.length === 0 ? <p class="muted small">None.</p> : list.map(t => <Card key={t.id} t={t} n={numbers.get(t.id)} {...p} file={file} now={now} />)}
    </section>
  );
  return (
    <aside class="sidebar" aria-label="Comment threads">
      {p.header}
      {section("section-open", "Open", [...attached, ...elsewhere])}
      {section("section-detached", "Detached", detached)}
      {section("section-resolved", "Resolved", done)}
    </aside>
  );
}

function Card({ t, n, now, me, selected, file, onSelect, onSend, onResolve, onReply, onHover }: Props & { t: Thread; n?: number; now: Date }) {
  const [reply, setReply] = useState("");
  const send = () => { if (reply.trim()) { onReply(t, reply); setReply(""); } };
  const label = t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null;
  return (
    <article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} onClick={() => onSelect(t)}
      onMouseEnter={() => onHover?.(t)} onMouseLeave={() => onHover?.(null)}>
      <header>
        <button type="button" class="card-head" aria-pressed={selected === t.id} onClick={e => { e.stopPropagation(); onSelect(t); }}>
          {n !== undefined && <span class="thread-num">{n}</span>}
          <span class="anchor-label">{anchorLabel(t.anchor)}</span>
          {t.anchor.file !== file && <span class="file-label muted small">on {t.anchor.file}</span>}
          <span class="muted small">v{t.version_n}</span>
        </button>
      </header>
      {t.clip_url && <img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />}
      {t.comments.map(c => (
        <div class={`comment ${c.author_kind === "agent" ? "agent" : "from-viewer"}`} key={c.id}>
          <div class="author">{c.author_kind === "agent" ? `Agent · via ${c.via_harness ?? c.author_name}` : c.author_name}{c.via_page && <span class="via-page muted small"> · via the page</span>}</div>
          <div class="body">{c.body}</div>
        </div>
      ))}
      {label && <p class="waiting">{label}</p>}
      {t.status === "resolved" && t.resolved_by && <p class="resolved-by muted small">Resolved by {resolvedByLabel(t.resolved_by, me)}</p>}
      {t.status === "open" && (
        <div class="actions" onClick={e => e.stopPropagation()}>
          {!t.sent_to_agent && <button class="primary" onClick={() => onSend(t)}>Send to agent</button>}
          <button onClick={() => onResolve(t)}>Resolve</button>
        </div>
      )}
      <form class="reply" onClick={e => e.stopPropagation()} onSubmit={e => { e.preventDefault(); send(); }}>
        <input aria-label="Reply" placeholder="Reply…" value={reply} onInput={e => setReply((e.target as HTMLInputElement).value)}
          onKeyDown={e => { if (isSubmitKey(e)) { e.preventDefault(); send(); } }} />
        <button type="submit">Reply</button>
      </form>
    </article>
  );
}
