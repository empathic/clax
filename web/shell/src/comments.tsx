import { type Anchor, type AnchorResult, INDEX_FILE } from "../../bridge/src/protocol";
import { useEffect, useRef, useState } from "preact/hooks";
import type { Thread } from "./threads";

/** A pick being commented on; `pickId` keys the composer so each pick starts empty. */
export type Draft = { pickId: string; anchor: Anchor; version: number; clip: Blob | null; clipError?: string };

/** Room a pin keeps from the stage's right edge: its own 22 px plus 16 px for a
 * classic scrollbar in the frame. */
export const PIN_RIGHT_ROOM = 38;

/** Numbered pins over the frame at the resolved rectangle of each attached
 * open thread on `file`, the page the frame shows (the index by default). */
export function Pins({ threads, resolved, onSelect, width, file = INDEX_FILE }: { threads: Thread[]; resolved: Record<string, AnchorResult>; onSelect(t: Thread): void; width?: number; file?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [measured, setMeasured] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el || width !== undefined) return;
    const measure = () => setMeasured(el.clientWidth);
    measure();
    if (typeof ResizeObserver === "function") {
      const ro = new ResizeObserver(measure);
      ro.observe(el);
      return () => ro.disconnect();
    }
    addEventListener("resize", measure);
    return () => removeEventListener("resize", measure);
  }, [width]);
  const stage = width ?? measured;
  // Numbered like the sidebar's Open section (open threads on this page not
  // detached); only those found with a rectangle get a pin, and a region
  // scrolled wholly above the frame gets none (one below it is clipped by
  // `.pins`). A pin never passes the stage's right edge or the frame's scrollbar.
  const attached = threads.filter(t => t.status === "open" && t.anchor.file === file && !(resolved[t.id] && !resolved[t.id].found));
  return (
    <div class="pins" ref={ref}>
      {attached.map((t, i) => {
        const r = resolved[t.id]?.rect;
        if (!r || r.y + r.h <= 0) return null;
        let left = r.x + r.w - 12;
        if (stage > 0) left = Math.min(left, stage - PIN_RIGHT_ROOM);
        return <button class="thread-pin" key={t.id} title={t.comments[0]?.body ?? ""} aria-label={`Thread ${i + 1}`} style={{ left: `${Math.max(0, left)}px`, top: `${Math.max(0, r.y - 12)}px` }} onClick={() => onSelect(t)}>{i + 1}</button>;
      })}
    </div>
  );
}

export function Composer({ draft, onCancel, onSubmit }: { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void> }) {
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [clipUrl, setClipUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!draft.clip) { setClipUrl(null); return; }
    const u = URL.createObjectURL(draft.clip);
    setClipUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [draft.clip]);
  const quote = draft.anchor.quote?.replace(/\s+/g, " ").trim();
  return (
    <form class="composer" onSubmit={async e => {
      e.preventDefault();
      if (!body.trim() || busy) return;
      setBusy(true);
      // `onSubmit` reports a failure in the stage's notice banner and rethrows;
      // the draft stays so the viewer can retry.
      try { await onSubmit(body); } catch { setBusy(false); }
    }}>
      <p class="composer-quote">{quote ? `«${quote.length > 160 ? `${quote.slice(0, 160)}…` : quote}»` : draft.anchor.selector}</p>
      {draft.anchor.file !== INDEX_FILE && <p class="file-label muted small">on {draft.anchor.file}</p>}

      {clipUrl ? <img class="clip" src={clipUrl} alt="Screenshot of the selected region" /> : <p class="muted small">No screenshot{draft.clipError ? `: ${draft.clipError}` : ""}</p>}
      <textarea autoFocus rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body} onInput={e => setBody((e.target as HTMLTextAreaElement).value)}
        onKeyDown={e => { if (e.key === "Escape") onCancel(); }} />
      <div class="actions">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="submit" class="primary" disabled={busy || !body.trim()}>Post comment</button>
      </div>
    </form>
  );
}
