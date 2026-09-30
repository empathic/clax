import { keyText } from "../../bridge/src/key-trap";
import { type Anchor, type AnchorResult, INDEX_FILE } from "../../bridge/src/protocol";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { type Thread, areaLabel } from "./threads";

/** A pick being commented on; `pickId` keys the composer so each pick starts
 * empty. `label` is a page's words for the spot, shown in place of the quote. */
/** `capturing`: its screenshot is still being taken, and arrives under
 * `clipToken` (`attachClip`). `early`: for a composer the viewer's own pick
 * opened, the keys they typed in the page before its textarea took focus
 * (`keyText`'s strings, in order), which come before anything typed in it;
 * `done` once no more can follow. */
export type Draft = { pickId: string; anchor: Anchor; version: number; clip: Blob | null; clipError?: string; label?: string; capturing?: boolean; clipToken?: string; early?: { keys: string[]; done: boolean } };

/** Most keys typed in the page that one pick's composer takes. */
export const MAX_EARLY_KEYS = 500;

/** How long a composer waits for a screenshot still being taken before it
 * says none was taken (the bridge's clip limit plus a margin; settable for tests). */
export const captureWait = { ms: 10_000 };
/** What a composer says when its screenshot never arrived. */
export const CAPTURE_LATE = "it was not taken in time";

/** Largest clip the daemon keeps, in bytes (its `MAX_CLIP_BYTES`). */
export const MAX_CLIP_BYTES = 5 * 1024 * 1024;

/** The composer after a page asks to open one for `d`: a fresh draft; null
 * (refused) when the open one holds typed text, unless `opts.area`, which
 * moves that composer, text kept (same `pickId`), to the new anchor. */
export function nextDraft(open: Draft | null, typed: string, d: Omit<Draft, "pickId">, opts?: { area?: boolean }, newId = () => `page-${Date.now()}-${Math.random().toString(36).slice(2)}`): Draft | null {
  if (open && typed.trim()) return opts?.area ? { ...d, pickId: open.pickId } : null;
  return { pickId: newId(), ...d };
}

/** The keys of a `clax:keys` message, at most `room` of them (extras are
 * dropped); null unless every one is a character (one code point, not a
 * control character), "\n", or "Backspace". */
export function earlyKeys(keys: unknown, room: number): string[] | null {
  if (!Array.isArray(keys) || keys.length > MAX_EARLY_KEYS) return null;
  const ok = keys.every(k => typeof k === "string" && (k === "\n" || k === "Backspace" || (Array.from(k).length === 1 && !/\p{Cc}/u.test(k))));
  return ok ? (keys as string[]).slice(0, Math.max(0, room)) : null;
}

/** `text` with `keys` typed at its end: each character or "\n" appended,
 * each "Backspace" removing the last character. */
export function typeKeys(text: string, keys: readonly string[]): string {
  const chars = Array.from(text);
  for (const k of keys) {
    if (k === "Backspace") chars.pop();
    else chars.push(k);
  }
  return chars.join("");
}

/** The draft with `keys` added to its early keys (and `done` set), when it is
 * the composer for `pickId` still taking them; else the draft unchanged. */
export function withEarly(dr: Draft | null, pickId: string, keys: string[], done: boolean): Draft | null {
  if (!dr || dr.pickId !== pickId || !dr.early || dr.early.done) return dr;
  return { ...dr, early: { keys: keys.length ? [...dr.early.keys, ...keys] : dr.early.keys, done } };
}

/** The draft with the clip taken for `token`, when it is still the one
 * waiting for it; else the draft unchanged. */
export function withClip(dr: Draft | null, token: string, clip: Blob | null, clipError?: string): Draft | null {
  return dr && dr.clipToken === token ? { ...dr, clip, clipError, capturing: false, clipToken: undefined } : dr;
}

/** Room a pin keeps from the stage's right edge: its own 22 px plus 16 px for a
 * classic scrollbar in the frame. */
export const PIN_RIGHT_ROOM = 38;

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
        return <button class="thread-pin" key={t.id} title={t.comments[0]?.body ?? ""} aria-label={`Thread ${i + 1}`} style={{ left: `${Math.max(0, left)}px`, top: `${Math.max(0, r.y - 12)}px` }} onClick={() => onSelect(t)}
          onMouseEnter={() => onHover?.(t)} onMouseLeave={() => onHover?.(null)}>{i + 1}</button>;
      })}
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

/** The composer for `draft`; `onText` hears the typed text on every input, and "" when it closes. */
export function Composer({ draft, onCancel, onSubmit, onText }: { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void>; onText?(text: string): void }) {
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
  useEffect(() => { textarea.current?.focus(); }, []);
  // The keys typed in the page after the pick come first (`draft.early`):
  // text keys typed here before they are all in are held, then added after
  // them. The text is only ever set here, never sent as input.
  const text = useRef("");
  const setText = (v: string) => { text.current = v; setBody(v); onTextRef.current?.(v); };
  const applied = useRef(0);
  const held = useRef<string[]>([]);
  const holding = useRef(!!draft.early && !draft.early.done);
  // A layout effect: it runs as the draft renders, so keys are held no longer
  // than it takes the last ones to arrive.
  useLayoutEffect(() => {
    const early = draft.early;
    if (!early) return;
    let next = typeKeys(text.current, early.keys.slice(applied.current));
    applied.current = early.keys.length;
    if (early.done) {
      holding.current = false;
      next = typeKeys(next, held.current);
      held.current = [];
    }
    if (next !== text.current) setText(next);
  }, [draft.early]);
  const quote = draft.anchor.quote?.replace(/\s+/g, " ").trim();
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
  // The submit shortcut while the screenshot is still being taken, or while
  // keys typed are held, posts once the screenshot is in (or the wait for it
  // ends) and the text holds every key: the composer opens at the pick, so a
  // viewer who types at once can press it before then.
  const [queued, setQueued] = useState(false);
  useEffect(() => {
    if (!queued || draft.capturing || holding.current || text.current !== body) return;
    setQueued(false);
    void post();
  }, [queued, draft.capturing, draft.early, body]);
  return (
    <form class="composer" onSubmit={e => { e.preventDefault(); void post(); }}>
      <p class="composer-quote">{draft.label ?? (quote ? `«${quote.length > 160 ? `${quote.slice(0, 160)}…` : quote}»` : draft.anchor.kind === "custom" ? draft.anchor.custom_name : draft.anchor.kind === "area" ? areaLabel(draft.anchor) : draft.anchor.selector)}</p>
      {draft.anchor.file !== INDEX_FILE && <p class="file-label muted small">on {draft.anchor.file}</p>}
      {clipUrl ? <img class="clip" src={clipUrl} alt="Screenshot of the selected region" /> : draft.capturing ? <p class="muted small">{queued ? "Posting once the screenshot is taken…" : "Taking the screenshot…"}</p> : <p class="muted small">No screenshot{draft.clipError ? `: ${draft.clipError}` : ""}</p>}
      <textarea ref={textarea} rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body} onInput={e => setText((e.target as HTMLTextAreaElement).value)}
        onKeyDown={e => {
          const typed = holding.current ? keyText(e) : null;
          if (typed !== null) { e.preventDefault(); held.current.push(typed); return; }
          if (e.key === "Escape") onCancel();
          else if (isSubmitKey(e)) {
            e.preventDefault();
            if (!busy && (holding.current || (draft.capturing && body.trim()))) setQueued(true);
            else void post();
          }
        }} />
      <div class="actions">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="submit" class="primary" title={`Post comment (${submitKeysLabel()})`} disabled={!canPost}>Post comment</button>
      </div>
    </form>
  );
}
