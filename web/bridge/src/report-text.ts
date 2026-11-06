// Formatting a report's anchor, browser and context rows as plain text:
// shared by the shell's queue pages (shell/src/queues/model.ts,
// ContextCard.svelte) and the overlay's own report views
// (overlay/src/ReportView.svelte, Reports.svelte), so a report reads the
// same whichever page shows it. Every function is pure and names no
// product (white label, Addendum 11): callers choose their own words
// around what these return.

/** What a report's anchor points at, as a line: the quote, the element's
 * name, the custom name or the selector, after the kind. `anchor` is
 * read as the wire sends it (untyped JSON), so it is checked, not cast. */
export function anchorText(anchor: unknown): string | null {
  if (!anchor || typeof anchor !== "object") return null;
  const a = anchor as { kind?: string; quote?: string | null; label?: string; name?: string; custom_name?: string | null; selector?: string | null };
  const kind = { element: "Element", range: "Text", area: "Area", custom: "Custom", artifact: "The whole page" }[a.kind ?? ""] ?? a.kind ?? "Anchor";
  const what = a.label || (a.quote ? `“${a.quote}”` : "") || a.name || a.custom_name || a.selector || "";
  return what ? `${kind}: ${what}` : kind;
}

/** A browser and its major version, read from a user agent: "Chrome 131",
 * "Safari 18", "Firefox 132", "Edge 131"; the agent itself when none is known. */
export function browserOf(ua: string | undefined): string | null {
  if (!ua) return null;
  const v = (re: RegExp) => re.exec(ua)?.[1];
  const known: [string, RegExp][] = [["Edge", /Edg(?:e|A|iOS)?\/(\d+)/], ["Firefox", /(?:Firefox|FxiOS)\/(\d+)/], ["Chrome", /(?:Chrome|CriOS)\/(\d+)/], ["Safari", /Version\/(\d+)[.\d]* .*Safari\//]];
  for (const [name, re] of known) { const n = v(re); if (n) return `${name} ${n}`; }
  return ua;
}

/** `iso` as a date and time in `zone` (the reader's when absent), with the
 * zone's short name; `null` for a zone the browser does not know or a time
 * it cannot read. */
export function timeIn(iso: string, zone?: string, locale?: string): string | null {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return null;
  try {
    return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", timeZone: zone }).format(t)
      + ` ${zoneName(t, zone, locale)}`;
  } catch { return null; }
}
function zoneName(t: Date, zone: string | undefined, locale: string | undefined): string {
  return new Intl.DateTimeFormat(locale, { timeZone: zone, timeZoneName: "short" }).formatToParts(t).find(p => p.type === "timeZoneName")?.value ?? zone ?? "";
}

/** A captured viewport as the Context card's "Viewport" row: its size, its
 * device pixel ratio when it is not 1, and the page's colour scheme when
 * known. */
export function viewportText(vp: { w: number; h: number; dpr: number } | undefined, colorScheme?: "light" | "dark"): string | null {
  if (!vp) return null;
  return `${vp.w} × ${vp.h}${vp.dpr !== 1 ? ` @${vp.dpr}x` : ""}${colorScheme ? ` · ${colorScheme}` : ""}`;
}

/** One of a few tones a state pill may be given, by state alone: a page
 * keeps its own words for the state (shell's STATE_LABEL, the overlay's
 * stateLabel) and its own colours per tone, so two pages can share this
 * mapping without sharing a look. */
export type StateTone = "info" | "progress" | "attention" | "done" | "neutral";
/** `state` is the SDK's `ReportState`, typed here as its five literals
 * alone, so this module adds no dependency on the SDK package. */
export function stateTone(state: "open" | "claimed" | "fix_proposed" | "verified" | "resolved"): StateTone {
  switch (state) {
    case "open": return "info";
    case "claimed": return "progress";
    case "fix_proposed": return "attention";
    case "verified": return "done";
    case "resolved": return "neutral";
  }
}
