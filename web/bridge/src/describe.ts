// What a picked element is, for a person: its tag, its ARIA role, its
// accessible name as the page gives them (`aria-label`, an image's `alt`, a
// `title`) and the text a reader sees in it. The shell's app mode checks them
// and turns them into words ("the button “Save”", "an image", “Mon 5 km”),
// never a selector; and the name an anchor records for its element at pick
// time (`nameOf`). Loaded with the comment part.
import { OVERLAY_TAG, spacedText, textIndex } from "./anchor";
import { KIND_WORDS, MAX_NAME, type PickDesc } from "./protocol";

/** Characters of an element's text its description keeps, at most. */
export const DESC_TEXT = 200;

/** `el`'s text for a label (`spacedText`), cut at `DESC_TEXT` characters.
 * The anchor's quote keeps the index's own text, which re-anchoring matches. */
const shownText = (el: Element): string | null => spacedText(textIndex(el), 0, Infinity, DESC_TEXT);

/** `el`'s tag, role and accessible name, as the page gives them, and its text. */
export function describeElement(el: Element): PickDesc {
  const a = (n: string) => el.getAttribute(n);
  return { tag: el.localName, role: a("role"), name: a("aria-label") ?? a("alt") ?? a("title"), text: shownText(el) };
}

/** Characters of an element's text, or of a heading, a name keeps, at most. */
export const NAME_TEXT = 60;
const HEADINGS = "h1,h2,h3,h4,h5,h6,[role=heading]";

/** `s` as one line of plain text: format characters dropped, control
 * characters, line separators and whitespace runs made one space, trimmed,
 * cut at `max` code points; null when nothing is left. */
function plain(s: string | null | undefined, max = NAME_TEXT): string | null {
  if (typeof s !== "string") return null;
  const t = s.replace(/\p{Cf}/gu, "").replace(/[\s\p{Cc}\u2028\u2029]+/gu, " ").trim();
  return t ? [...t].slice(0, max).join("").trimEnd() : null;
}

/** `el`'s text as a reader sees it, cut at `NAME_TEXT` characters. */
const textOf = (el: Element): string | null => plain(spacedText(textIndex(el), 0, Infinity, NAME_TEXT));

/** `el`'s accessible name as the page gives it: `aria-label`, the text of
 * the elements `aria-labelledby` names, `alt`, `title`, a table's caption or
 * a figure's caption. */
function accessibleName(el: Element): string | null {
  const a = (n: string) => plain(el.getAttribute(n));
  const by = el.getAttribute("aria-labelledby")?.split(/\s+/).map(id => (id ? el.ownerDocument.getElementById(id) : null)).filter((e): e is HTMLElement => !!e);
  const caption = el.localName === "table" ? el.querySelector(":scope > caption") : el.localName === "figure" ? el.querySelector(":scope > figcaption") : null;
  return a("aria-label") ?? (by?.length ? plain(by.map(textOf).filter(Boolean).join(" ")) : null) ?? a("alt") ?? a("title") ?? (caption ? textOf(caption) : null);
}

/** The heading nearest before `el` in document order (not inside it, nor
 * holding it), outside Clax's overlay; null when there is none. */
function headingBefore(el: Element): Element | null {
  let best: Element | null = null;
  for (const h of el.ownerDocument.querySelectorAll(HEADINGS)) {
    if (!(h.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING)) break;
    if (!h.contains(el) && !h.closest(OVERLAY_TAG)) best = h;
  }
  return best;
}

/** What `el` is called, for a thread's title: its accessible name, else the
 * first `NAME_TEXT` characters of its text, else its kind word
 * (`KIND_WORDS`, by role, then tag) with the nearest heading before it
 * ("Chart under ‘p95 by four-hour window’"), or the kind word alone; null
 * when it has none of them. One line of plain text, at most `MAX_NAME`
 * characters. */
export function nameOf(el: Element): string | null {
  const own = accessibleName(el) ?? textOf(el);
  if (own) return own;
  const role = el.getAttribute("role")?.trim().split(/\s+/)[0]?.toLowerCase();
  const kind = (role && Object.hasOwn(KIND_WORDS, role) ? KIND_WORDS[role] : null) ?? (Object.hasOwn(KIND_WORDS, el.localName) ? KIND_WORDS[el.localName] : null);
  if (!kind) return null;
  const h = headingBefore(el);
  const heading = h ? textOf(h) : null;
  return heading ? plain(`${kind} under ‘${heading}’`, MAX_NAME) : kind;
}
