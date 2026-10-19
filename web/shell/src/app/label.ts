// What an anchor is about, in words a person reads in the app, as a
// thread's title: the text it quotes, else the name the bridge recorded for
// its element at pick time, else what the element is ("Table", "Chart"),
// else "A spot on the page"; "Whole app" for the whole artifact. Never a
// selector: one is read, at most, for its last tag name. The page's own
// words (a quote, a name, a heading) are always inside “ ”, with format,
// control and bidi characters dropped and quote marks inside them made ', so
// no page text can read as the app's own words or close its quotes; only the
// app's own kind words (`KIND_WORDS`) stand outside them.
import { type Anchor, KIND_WORDS, type PickDesc } from "../../../bridge/src/protocol";
import { cleanLabel } from "../caps/comments";
import { respaced } from "../view/composer-model";

/** The longest quote, name or heading shown, in characters. */
const QUOTE_CHARS = 60;
/** The longest custom anchor name shown, in characters. */
const NAME_CHARS = 40;
/** The longest quote a draft carries for the app's preview of what a
 * comment is on, in characters. */
export const PREVIEW_CHARS = 280;
const QUOTE_MARKS = /["“”„‟«»‹›＂〝〞〟‚]/g;
const KINDS = new Set(Object.values(KIND_WORDS));

/** The page's text `s`, made safe to fence in “ ”: `cleanLabel`'s rules
 * (no format or control characters, collapsed spaces, some letter or digit),
 * quote marks made ', cut at `max` characters; null when nothing is left. */
const clean = (s: unknown, max: number): string | null => {
  if (typeof s !== "string") return null;
  const t = cleanLabel(s)?.replace(QUOTE_MARKS, "'");
  if (!t) return null;
  const chars = [...t];
  return chars.length > max ? `${chars.slice(0, max).join("").trimEnd()}…` : t;
};
const word = (s: unknown): string | null => (typeof s === "string" && /^[a-z][a-z0-9-]{0,31}$/i.test(s.trim()) ? s.trim().toLowerCase() : null);

/** The last tag name in `selector` (`body > main > h2.x:nth-of-type(2)` → `h2`). */
function tagOf(selector: string | null): string | null {
  const last = selector?.split(/[\s>+~]+/).filter(Boolean).at(-1);
  return word(last?.match(/^[a-z][a-z0-9-]*/i)?.[0] ?? null);
}

/** What an element is called, in parts: a kind word of the app's own
 * (`KIND_WORDS`) with the heading before it, or the page's `own` name. */
type Named = { kind: string; heading?: string } | { own: string };

/** A name from the bridge (`Anchor.name`, or a description's name) read as
 * `Named`: "<kind word> under ‘<heading>’", a kind word alone, or else the
 * page's own words. */
function namedAs(name: unknown): Named | null {
  if (typeof name !== "string") return null;
  const t = name.trim();
  const m = /^(.+?) under ‘(.+)’$/su.exec(t);
  if (m && KINDS.has(m[1])) {
    const heading = clean(m[2], QUOTE_CHARS);
    return heading ? { kind: m[1], heading } : { kind: m[1] };
  }
  if (KINDS.has(t)) return { kind: t };
  const own = clean(name, QUOTE_CHARS);
  return own ? { own } : null;
}

/** The kind word for role `role`, else for tag `tag`. */
function kindOf(tag: string | null, role: string | null): Named | null {
  const of = (k: string | null) => (k && Object.hasOwn(KIND_WORDS, k) ? KIND_WORDS[k] : null);
  const kind = of(role) ?? of(tag);
  return kind ? { kind } : null;
}

/** `n` as a title ("“Save”", "Chart under “p95”", "Chart"), or, for a drawn
 * area, as the place it is on ("Area on “Save”", "Area on the chart under
 * “p95”"). */
function titled(n: Named, area: boolean): string {
  if ("own" in n) return area ? `Area on “${n.own}”` : `“${n.own}”`;
  const what = n.heading ? `${n.kind} under “${n.heading}”` : n.kind;
  return area ? `Area on the ${what[0].toLowerCase()}${what.slice(1)}` : what;
}

/** The text `anchor` quotes as a reader sees it (its `label`, else the
 * description's spaced `text`, else the quote), for the app to show in its
 * own quote marks: format and control characters dropped, spaces
 * collapsed, quote marks made ', cut at `max` characters with "…"; null
 * when it quotes nothing with a letter or digit. */
export function anchorQuote(anchor: Anchor, desc?: Partial<PickDesc> | null, max = PREVIEW_CHARS): string | null {
  const d = desc && typeof desc === "object" ? desc : null;
  const raw = respaced(anchor.quote, anchor.label) ?? respaced(anchor.quote, d?.text) ?? anchor.quote;
  if (typeof raw !== "string") return null;
  const t = raw.replace(/\p{Cf}/gu, "").replace(/\s+/g, " ").replace(/\p{Cc}/gu, "").trim().replace(QUOTE_MARKS, "'");
  if (!/[\p{L}\p{N}]/u.test(t)) return null;
  const chars = [...t];
  return chars.length > max ? `${chars.slice(0, max).join("").trimEnd()}…` : t;
}

/** The words for `anchor`, a thread's title: its quote (the anchor's
 * `label`, else the description's `text`, the quote spaced between
 * elements, in its place), a custom anchor's name, the name the bridge
 * recorded at pick time (`Anchor.name`), the name in the bridge's
 * description of a picked element (`desc`, the page's own and so checked
 * here), the element's kind word, else "A spot on the page"; a drawn area
 * reads "Area on …", else "Area on the page". */
export function anchorLabel(anchor: Anchor, desc?: Partial<PickDesc> | null): string {
  if (anchor.kind === "artifact") return "Whole app";
  const d = desc && typeof desc === "object" ? desc : null;
  const quote = clean(respaced(anchor.quote, anchor.label) ?? respaced(anchor.quote, d?.text), QUOTE_CHARS) ?? clean(anchor.quote, QUOTE_CHARS);
  if (quote) return `“${quote}”`;
  if (anchor.kind === "custom") {
    const n = clean(anchor.custom_name, NAME_CHARS);
    if (n) return `“${n}”`;
  }
  const area = anchor.kind === "area";
  const tag = word(d?.tag) ?? word(anchor.area?.tag) ?? tagOf(anchor.selector);
  const n = namedAs(anchor.name) ?? namedAs(d?.name) ?? kindOf(tag, word(d?.role));
  if (n) return titled(n, area);
  return area ? "Area on the page" : "A spot on the page";
}
