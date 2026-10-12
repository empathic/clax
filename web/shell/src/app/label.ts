// What an anchor is about, in words a person reads in the app: the text it
// quotes, else what the element is ("the button “Save”", "a heading", "an
// image"), else "this spot"; "Whole app" for the whole artifact. Never a
// selector: one is read, at most, for its last tag name. The page's own words
// (a quote, a name) are always inside “ ”, with format, control and bidi
// characters dropped and quote marks inside them made ', so no page text can
// read as the app's own words or close its quotes.
import type { Anchor, PickDesc } from "../../../bridge/src/protocol";
import { cleanLabel } from "../caps/comments";

/** The longest quote shown, in characters. */
const QUOTE_CHARS = 60;
/** The longest element name shown, in characters. */
const NAME_CHARS = 40;

/** The noun for an element of tag or role `w`, with its article: "a" for an
 * unnamed one, the noun alone for a named one ("the " is added). */
const NOUNS: Record<string, string> = {
  button: "button", a: "link", link: "link", img: "image", image: "image", svg: "image", picture: "image", figure: "figure",
  video: "video", audio: "audio player", canvas: "drawing", input: "field", textarea: "text box", textbox: "text box", select: "menu",
  combobox: "menu", checkbox: "checkbox", radio: "option", option: "option", switch: "switch", slider: "slider", heading: "heading",
  h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading", p: "paragraph", ul: "list", ol: "list",
  list: "list", li: "list item", listitem: "list item", table: "table", grid: "table", tr: "table row", row: "table row",
  td: "table cell", th: "table cell", cell: "table cell", nav: "navigation", navigation: "navigation", header: "header",
  banner: "header", footer: "footer", form: "form", dialog: "dialog", tab: "tab", menu: "menu", menuitem: "menu item",
  label: "label", blockquote: "quote", pre: "code block", code: "code", progress: "progress bar", progressbar: "progress bar",
};

/** The page's text `s`, made safe to fence in “ ”: `cleanLabel`'s rules
 * (no format or control characters, collapsed spaces, some letter or digit),
 * quote marks made ', cut at `max` characters; null when nothing is left. */
const clean = (s: unknown, max: number): string | null => {
  if (typeof s !== "string") return null;
  const t = cleanLabel(s)?.replace(/["“”„‟«»‹›＂〝〞〟‚]/g, "'");
  if (!t) return null;
  const chars = [...t];
  return chars.length > max ? `${chars.slice(0, max).join("").trimEnd()}…` : t;
};
const word = (s: unknown): string | null => (typeof s === "string" && /^[a-z][a-z0-9-]{0,31}$/i.test(s.trim()) ? s.trim().toLowerCase() : null);
const article = (n: string) => (/^[aeiou]/.test(n) ? "an" : "a");

/** The last tag name in `selector` (`body > main > h2.x:nth-of-type(2)` → `h2`). */
function tagOf(selector: string | null): string | null {
  const last = selector?.split(/[\s>+~]+/).filter(Boolean).at(-1);
  return word(last?.match(/^[a-z][a-z0-9-]*/i)?.[0] ?? null);
}

/** What the element is: from its role, else its tag, and its name. */
function elementWords(tag: string | null, role: string | null, name: string | null): string | null {
  const noun = (role && NOUNS[role]) || (tag && NOUNS[tag]) || null;
  if (noun) return name ? `the ${noun} “${name}”` : `${article(noun)} ${noun}`;
  return name ? `“${name}”` : null;
}

/** The words for `anchor`, with the bridge's description of a picked
 * element when there is one (`desc`, the page's own and so checked here). */
export function anchorLabel(anchor: Anchor, desc?: PickDesc | null): string {
  if (anchor.kind === "artifact") return "Whole app";
  const quote = clean(anchor.quote, QUOTE_CHARS);
  if (quote) return `“${quote}”`;
  if (anchor.kind === "custom") {
    const n = clean(anchor.custom_name, NAME_CHARS);
    if (n) return `“${n}”`;
  }
  const d = desc && typeof desc === "object" ? desc : null;
  const tag = word(d?.tag) ?? word(anchor.area?.tag) ?? tagOf(anchor.selector);
  const words = elementWords(tag, word(d?.role), clean(d?.name, NAME_CHARS));
  if (anchor.kind === "area") return words ? `part of ${words}` : "this area";
  return words ?? "this spot";
}
