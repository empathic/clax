// What a picked element is, for a person: its tag, its ARIA role, its
// accessible name as the page gives them (`aria-label`, an image's `alt`, a
// `title`) and the text a reader sees in it. The shell's app mode checks them
// and turns them into words ("the button “Save”", "an image", “Mon 5 km”),
// never a selector. Loaded with the comment part.
import { textIndex } from "./anchor";
import type { PickDesc } from "./protocol";

/** Characters of an element's text its description keeps, at most. */
export const DESC_TEXT = 200;

/** `el`'s text for a label: the text nodes its quote is cut from, with a
 * space between two that belong to different elements (unless the join has
 * one, or the second opens with closing punctuation), whitespace collapsed
 * and trimmed, cut at `DESC_TEXT` characters; null when it has none. The
 * anchor's quote keeps the index's own text, which re-anchoring matches. */
function shownText(el: Element): string | null {
  let out = "";
  let prev: Element | null = null;
  for (const { node } of textIndex(el).pieces) {
    const parent = node.parentElement;
    out += (prev && parent !== prev && !/^[\s.,;:!?)\]}]/.test(node.data) ? " " : "") + node.data;
    prev = parent;
    if (out.length > 4 * DESC_TEXT) break;
  }
  const t = [...out.replace(/\s+/g, " ").trim()].slice(0, DESC_TEXT).join("");
  return t || null;
}

/** `el`'s tag, role and accessible name, as the page gives them, and its text. */
export function describeElement(el: Element): PickDesc {
  const a = (n: string) => el.getAttribute(n);
  return { tag: el.localName, role: a("role"), name: a("aria-label") ?? a("alt") ?? a("title"), text: shownText(el) };
}
