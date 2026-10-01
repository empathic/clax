import { type Anchor, type AnchorResult, INDEX_FILE } from "../../bridge/src/protocol";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Thread } from "./threads";
import { type Draft, composerQuote } from "./view/composer-model";
import { pinPlaces } from "./view/pins-model";

/** Numbered pins over the frame at the top right of the resolved rectangle
 * (for an area thread, the drawn area) of each attached open thread on
 * `file`, the page the frame shows (the index by default; none when it is
 * null, a document that did not greet). `onHover` hears the thread whose pin
 * the pointer is over, and null when it leaves. */
export function Pins({ threads, resolved, onSelect, onHover, width, file = INDEX_FILE }: { threads: Thread[]; resolved: Record<string, AnchorResult>; onSelect(t: Thread): void; onHover?(t: Thread | null): void; width?: number; file?: string | null }) {
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
  // A region scrolled wholly above the frame gets no pin; one below it is
  // clipped by `.pins`.
  return (
    <div class="pins" ref={ref}>
      {pinPlaces(threads, resolved, file, stage).map(p => (
        <button class="thread-pin" key={p.thread.id} title={p.thread.comments[0]?.body ?? ""} aria-label={`Thread ${p.n}`} style={{ left: `${p.left}px`, top: `${p.top}px` }} onClick={() => onSelect(p.thread)}
          onMouseEnter={() => onHover?.(p.thread)} onMouseLeave={() => onHover?.(null)}>{p.n}</button>
      ))}
    </div>
  );
}

/** Whether `e` is the shortcut that posts a comment: Enter with Cmd or Ctrl
 * (either, on every platform), and not while an IME composition is in progress. */
export function isSubmitKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "isComposing">): boolean {
  return e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing;
}

/** The submit shortcut as `platform` names it: "⌘↵" on Apple platforms, "Ctrl+Enter" elsewhere. */
export function submitKeysLabel(platform = typeof navigator === "undefined" ? "" : navigator.platform): string {
  return /Mac|iPhone|iPad|iPod/.test(platform) ? "⌘↵" : "Ctrl+Enter";
}

/** The composer for `draft`; `onText` hears the typed text on every input, and "" when it closes; `onFocused` hears that its textarea took focus as it opened. */
export function Composer({ draft, onCancel, onSubmit, onText, onFocused }: { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void>; onText?(text: string): void; onFocused?(): void }) {
  const [body, setBody] = useState("");
  const onTextRef = useRef(onText);
  onTextRef.current = onText;
  useEffect(() => () => onTextRef.current?.(""), []);
  const [busy, setBusy] = useState(false);
  const [clipUrl, setClipUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!draft.clip) { setClipUrl(null); return; }
    const u = URL.createObjectURL(draft.clip);
    setClipUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [draft.clip]);
  // The viewer types at once: focus moves from the page to the textarea when
  // the composer opens (a script focus, not the viewer's input to the shell).
  const textarea = useRef<HTMLTextAreaElement>(null);
  const onFocusedRef = useRef(onFocused);
  onFocusedRef.current = onFocused;
  // A layout effect: focus moves as the composer is first rendered, not after
  // the next paint, so the hand-off from the page is as short as it can be.
  useLayoutEffect(() => { textarea.current?.focus(); onFocusedRef.current?.(); }, []);
  const canPost = !busy && !!body.trim() && !draft.capturing;
  // Set before the first await, so a second Post or shortcut in the same
  // render cannot post twice.
  const posting = useRef(false);
  const post = async () => {
    if (!canPost || posting.current) return;
    posting.current = true;
    setBusy(true);
    // `onSubmit` reports a failure in the stage's notice banner and rethrows;
    // the draft stays so the viewer can retry.
    try { await onSubmit(body); } catch { posting.current = false; setBusy(false); }
  };
  // Post (a click, Enter on it, or the shortcut) while the screenshot is
  // still being taken posts once it is in (or the wait for it ends): the
  // composer opens at the pick, so a viewer who types at once can post before
  // then. It posts the text the textarea showed when it was asked, on the
  // anchor shown then: an edit after it, or a move of the composer to
  // another anchor (a page's area), cancels it, and the viewer posts again.
  const [queued, setQueued] = useState(false);
  const queuedFor = useRef<Anchor | null>(null);
  useEffect(() => {
    if (queued && queuedFor.current !== draft.anchor) setQueued(false);
  }, [draft.anchor]);
  useEffect(() => {
    if (!queued || draft.capturing) return;
    setQueued(false);
    // Only on the anchor, and with the text, shown when it was asked.
    if (queuedFor.current === draft.anchor && textarea.current?.value === body) void post();
  }, [queued, draft.capturing, draft.anchor]);
  /** Posts now, or once the screenshot is in. */
  const request = () => {
    if (!draft.capturing) { void post(); return; }
    if (busy || !body.trim()) return;
    queuedFor.current = draft.anchor;
    setQueued(true);
  };
  return (
    <form class="composer" onSubmit={e => { e.preventDefault(); request(); }}>
      <p class="composer-quote">{composerQuote(draft)}</p>
      {draft.anchor.file !== INDEX_FILE && <p class="file-label muted small">on {draft.anchor.file}</p>}
      {clipUrl ? <img class="clip" src={clipUrl} alt="Screenshot of the selected region" /> : draft.capturing ? <p class="muted small">{queued ? "Posting once the screenshot is taken…" : "Taking the screenshot…"}</p> : <p class="muted small">No screenshot{draft.clipError ? `: ${draft.clipError}` : ""}</p>}
      <textarea ref={textarea} rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body} onInput={e => { const v = (e.target as HTMLTextAreaElement).value; setQueued(false); setBody(v); onText?.(v); }}
        onKeyDown={e => {
          if (e.key === "Escape") onCancel();
          else if (isSubmitKey(e)) {
            e.preventDefault();
            request();
          }
        }} />
      <div class="actions">
        <button type="button" onClick={onCancel}>Cancel</button>
        {/* While the screenshot is taken, Post stays focusable (Tab order
            does not change) and waits: pressing it queues the post. */}
        <button type="submit" class="primary" title={draft.capturing ? "Posts once the screenshot is taken" : `Post comment (${submitKeysLabel()})`} disabled={busy || !body.trim()}
          aria-disabled={draft.capturing ? "true" : undefined}>Post comment</button>
      </div>
    </form>
  );
}
