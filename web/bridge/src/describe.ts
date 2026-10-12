// What a picked element is, for a person: its tag, its ARIA role and its
// accessible name as the page gives them (`aria-label`, an image's `alt`, a
// `title`). The shell's app mode checks them and turns them into words ("the
// button “Save”", "an image"), never a selector. Loaded with the comment part.
import type { PickDesc } from "./protocol";

/** `el`'s tag, role and accessible name, as the page gives them. */
export function describeElement(el: Element): PickDesc {
  const a = (n: string) => el.getAttribute(n);
  return { tag: el.localName, role: a("role"), name: a("aria-label") ?? a("alt") ?? a("title") };
}
