import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import type { AnchorResult } from "../../bridge/src/protocol";
import { anchorLabel, type Thread } from "./threads";
import { waitingLabel } from "./waiting";

type Props = {
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  now: Date;
  selected: string | null;
  onSelect(t: Thread): void;
  onSend(t: Thread): void;
  onResolve(t: Thread): void;
  onReply(t: Thread, body: string): void;
  /** Rendered above the sections (the "Your name" field on narrow screens). */
  header?: ComponentChildren;
};

/** Open threads found on this version, open threads not found (Detached), then resolved threads. */
export function Sidebar(p: Props) {
  const open = p.threads.filter(t => t.status === "open");
  const detached = open.filter(t => p.resolved[t.id] && !p.resolved[t.id].found);
  const attached = open.filter(t => !detached.includes(t));
  const done = p.threads.filter(t => t.status === "resolved");
  const numbers = new Map(attached.map((t, i) => [t.id, i + 1]));
  const section = (cls: string, title: string, list: Thread[]) => (
    <section class={cls}>
      <h2>{title} <span class="muted">{list.length}</span></h2>
      {list.length === 0 ? <p class="muted small">None.</p> : list.map(t => <Card key={t.id} t={t} n={numbers.get(t.id)} {...p} />)}
    </section>
  );
  return (
    <aside class="sidebar" aria-label="Comment threads">
      {p.header}
      {section("section-open", "Open", attached)}
      {section("section-detached", "Detached", detached)}
      {section("section-resolved", "Resolved", done)}
    </aside>
  );
}

function Card({ t, n, now, selected, onSelect, onSend, onResolve, onReply }: Props & { t: Thread; n?: number }) {
  const [reply, setReply] = useState("");
  const label = t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null;
  return (
    <article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} onClick={() => onSelect(t)}>
      <header>
        {n !== undefined && <span class="thread-num">{n}</span>}
        <span class="anchor-label">{anchorLabel(t.anchor)}</span>
        <span class="muted small">v{t.version_n}</span>
      </header>
      {t.clip_url && <img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />}
      {t.comments.map(c => (
        <div class={`comment ${c.author_kind === "agent" ? "agent" : "from-viewer"}`} key={c.id}>
          <div class="author">{c.author_kind === "agent" ? `Agent · via ${c.author_name}` : c.author_name}</div>
          <div class="body">{c.body}</div>
        </div>
      ))}
      {label && <p class="waiting">{label}</p>}
      {t.status === "open" && (
        <div class="actions" onClick={e => e.stopPropagation()}>
          {!t.sent_to_agent && <button class="primary" onClick={() => onSend(t)}>Send to agent</button>}
          <button onClick={() => onResolve(t)}>Resolve</button>
        </div>
      )}
      <form class="reply" onClick={e => e.stopPropagation()} onSubmit={e => { e.preventDefault(); if (reply.trim()) { onReply(t, reply); setReply(""); } }}>
        <input aria-label="Reply" placeholder="Reply…" value={reply} onInput={e => setReply((e.target as HTMLInputElement).value)} />
        <button type="submit">Reply</button>
      </form>
    </article>
  );
}
