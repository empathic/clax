import type { Anchor, AnchorResult } from "../../bridge/src/protocol";
import { useEffect, useState } from "preact/hooks";
import type { Thread } from "./threads";

export type Draft = { anchor: Anchor; version: number; clip: Blob | null; clipError?: string };

/** Numbered pins over the frame at each attached open thread's resolved rectangle. */
export function Pins({ threads, resolved, onSelect }: { threads: Thread[]; resolved: Record<string, AnchorResult>; onSelect(t: Thread): void }) {
  // Numbered like the sidebar's Open section (open threads not detached); only
  // those found with a rectangle get a pin, and a region scrolled wholly above
  // the frame gets none (one below it is clipped by `.pins`).
  const attached = threads.filter(t => t.status === "open" && !(resolved[t.id] && !resolved[t.id].found));
  return (
    <div class="pins">
      {attached.map((t, i) => {
        const r = resolved[t.id]?.rect;
        if (!r || r.y + r.h <= 0) return null;
        return <button class="thread-pin" key={t.id} title={t.comments[0]?.body ?? ""} aria-label={`Thread ${i + 1}`} style={{ left: `${Math.max(0, r.x + r.w - 12)}px`, top: `${Math.max(0, r.y - 12)}px` }} onClick={() => onSelect(t)}>{i + 1}</button>;
      })}
    </div>
  );
}

export function Composer({ draft, onCancel, onSubmit }: { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void> }) {
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
      setError(null);
      try { await onSubmit(body); } catch (err) { setError(String(err)); setBusy(false); }
    }}>
      <p class="composer-quote">{quote ? `«${quote.length > 160 ? `${quote.slice(0, 160)}…` : quote}»` : draft.anchor.selector}</p>
      {clipUrl ? <img class="clip" src={clipUrl} alt="Screenshot of the selected region" /> : <p class="muted small">No screenshot{draft.clipError ? `: ${draft.clipError}` : ""}</p>}
      <textarea autoFocus rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body} onInput={e => setBody((e.target as HTMLTextAreaElement).value)}
        onKeyDown={e => { if (e.key === "Escape") onCancel(); }} />
      {error && <p class="error small">{error}</p>}
      <div class="actions">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="submit" class="primary" disabled={busy || !body.trim()}>Post comment</button>
      </div>
    </form>
  );
}
