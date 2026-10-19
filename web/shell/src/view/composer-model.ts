import type { Anchor, Box } from "../../../bridge/src/protocol";
import { areaLabel, respaced } from "../threads";

export { respaced };

/** A pick being commented on; `pickId` keys the composer so each pick starts
 * empty. `label` is a page's words for the spot, shown in place of the quote. */
/** `capturing`: its screenshot is still being taken, and arrives under
 * `clipToken` (`attachClip`). */
/** `shown`: the picked element's text spaced between elements (the bridge's
 * `PickDesc.text`), shown in place of its run-together quote. */
/** `byPage`: the page opened (or moved) this composer through the `comments`
 * capability, not the viewer's own pick, so its Post takes only a pointer's
 * click (`guardedAction`). */
/** `at`: where the bridge said the pick is in the frame's viewport, CSS
 * pixels (a rectangle of finite numbers, never negative in size). */
export type Draft = { pickId: string; anchor: Anchor; version: number; clip: Blob | null; clipError?: string; label?: string; capturing?: boolean; clipToken?: string; byPage?: boolean; shown?: string; at?: Box };

/** `v` when it is a rectangle {x, y, w, h} of finite numbers whose size is
 * not negative, else undefined. */
export function boxOf(v: unknown): Box | undefined {
  const b = (v ?? null) as Partial<Record<keyof Box, unknown>> | null;
  if (!b || typeof b !== "object") return undefined;
  const { x, y, w, h } = b;
  const ok = [x, y, w, h].every(n => typeof n === "number" && Number.isFinite(n)) && (w as number) >= 0 && (h as number) >= 0;
  return ok ? { x: x as number, y: y as number, w: w as number, h: h as number } : undefined;
}

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

/** The draft with the clip taken for `token`, when it is still the one
 * waiting for it; else the draft unchanged. */
export function withClip(dr: Draft | null, token: string, clip: Blob | null, clipError?: string): Draft | null {
  return dr && dr.clipToken === token ? { ...dr, clip, clipError, capturing: false, clipToken: undefined } : dr;
}

/** What the composer shows for its target: the page's label, else the quote
 * (its anchor's `label`, else its `shown` text, when it has one; whitespace collapsed, cut at 160 characters), else the custom anchor's
 * name, an area's label, or the element's selector. */
export function composerQuote(draft: Draft): string {
  const q = draft.anchor.quote;
  const quote = (respaced(q, draft.anchor.label) ?? respaced(q, draft.shown) ?? q)?.replace(/\s+/g, " ").trim();
  if (draft.label !== undefined) return draft.label;
  if (quote) return `«${quote.length > 160 ? `${quote.slice(0, 160)}…` : quote}»`;
  if (draft.anchor.kind === "custom") return draft.anchor.custom_name ?? "";
  if (draft.anchor.kind === "area") return areaLabel(draft.anchor);
  return draft.anchor.selector ?? "";
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
