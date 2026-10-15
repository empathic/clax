// What a picked element is, for a person: its tag, its ARIA role, its
// accessible name as the page gives them (`aria-label`, an image's `alt`, a
// `title`) and the text a reader sees in it. The shell's app mode checks them
// and turns them into words ("the button “Save”", "an image", “Mon 5 km”),
// never a selector. Loaded with the comment part.
import { spacedText, textIndex } from "./anchor";
import type { PickDesc } from "./protocol";

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
