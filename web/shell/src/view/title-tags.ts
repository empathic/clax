// The artifact's tags beside its by-line (docs/contract.md "Tags"): each a
// link to the gallery filtered by it where the viewer can reach the gallery
// (`reach`; a hosted Clax's guest is sent to sign in), else a plain chip.
// The chips are clipped to one line: "+N" opens the whole list
// (`ui/tag-editor.ts` `toggleTagList`), and at phone width, where the chips
// give way, "N tags" does, for every viewer. Then, for a viewer who may set
// them (`may_tag`), Edit tags (Add tags while it carries none), which opens
// the tag editor (`ui/tag-editor.ts`, its own chunk). Plain DOM, as the
// by-line is, so the artifact entry carries no component for it.
import type { TagRef } from "../api";
import type { ViewState } from "./artifact-controller";

/** The artifact's tags and whether the viewer may set them: as last
 * refetched (`tags`), else as loaded. */
export const tagsOf = (s: ViewState): { tags: TagRef[]; may: boolean } => s.tags ?? { tags: s.data?.artifact.tags ?? [], may: !!s.data?.artifact.may_tag };

/** Tag `t` as a chip: a link to the gallery filtered by it when `reach`, else a span. */
export function tagChip(doc: Document, t: TagRef, reach: boolean): HTMLElement {
  const c = doc.createElement(reach ? "a" : "span");
  c.className = "tag";
  c.dataset.c = t.colour;
  c.textContent = t.name;
  if (c instanceof HTMLAnchorElement) {
    c.href = `/?tag=${encodeURIComponent(t.name)}`;
    c.title = `The gallery, filtered by tag ${t.name}`;
  }
  return c;
}

/** Whether this viewer can open the gallery: `/` answers it, rather than sending it to sign in. */
export const reachesGallery = (): Promise<boolean> => fetch("/", { redirect: "manual" }).then(r => r.ok, () => false);

const button = (doc: Document, cls: string, on: (b: HTMLButtonElement) => void): HTMLButtonElement => {
  const b = doc.createElement("button");
  b.type = "button";
  b.className = cls;
  b.setAttribute("aria-haspopup", "dialog");
  b.setAttribute("aria-expanded", "false");
  b.onclick = () => on(b);
  return b;
};
const span = (doc: Document, cls: string, text: string): HTMLSpanElement => {
  const s = doc.createElement("span");
  s.className = cls;
  s.textContent = text;
  return s;
};

/** Hides the chips of `box` that do not fit on its line, last first, and
 * says how many in its "+N" (`.tmore[data-over]`). A no-op where the chips
 * are not laid out (at phone width, or with no layout at all). */
export function fitTitleTags(box: HTMLElement): void {
  const chips = box.querySelector<HTMLElement>(":scope > .tchips");
  const more = box.querySelector<HTMLElement>(":scope > .tmore");
  if (!chips || !more) return;
  const all = [...chips.children] as HTMLElement[];
  for (const c of all) c.hidden = false;
  more.removeAttribute("data-over");
  let over = 0;
  for (let i = all.length - 1; i > 0 && chips.scrollWidth > chips.clientWidth + 0.5; i--) {
    all[i].hidden = true;
    over++;
    more.dataset.over = String(over);
    more.querySelector(".tm-d")!.textContent = `+${over}`;
  }
}

/** Keeps `ttl`'s tags (`.ttags`, made when first needed) showing `tags`,
 * linked when `reach`, with the edit button when `may`; `list` gets the
 * "+N" or "N tags" button when pressed, and `edit` the edit button. Touches
 * the DOM only when they change. */
export function setTitleTags(ttl: HTMLElement, tags: readonly TagRef[], may: boolean, reach: boolean, list: (b: HTMLButtonElement) => void, edit: (b: HTMLButtonElement) => void): void {
  let box = ttl.querySelector<HTMLElement>(":scope > .ttags");
  if (!tags.length && !may) { box?.remove(); return; }
  const key = JSON.stringify([tags, may, reach]);
  if (box?.dataset.k === key) return;
  const doc = ttl.ownerDocument;
  if (!box) {
    box = span(doc, "ttags", "");
    box.append(span(doc, "tchips", ""));
    ttl.append(box);
  }
  box.dataset.k = key;
  box.firstElementChild!.replaceChildren(...tags.map(t => tagChip(doc, t, reach)));
  let more = box.querySelector<HTMLButtonElement>(":scope > .tmore");
  if (tags.length) {
    if (!more) {
      more = button(doc, "tmore", list);
      box.firstElementChild!.after(more);
    }
    const many = `${tags.length} tag${tags.length === 1 ? "" : "s"}`;
    more.setAttribute("aria-label", `All ${many}`);
    more.title = `All ${many}`;
    more.replaceChildren(span(doc, "tm-d", ""), span(doc, "tm-p", many));
  } else more?.remove();
  let b = box.querySelector<HTMLButtonElement>(":scope > .tedit");
  if (may) {
    b ??= box.appendChild(button(doc, "tedit", edit));
    b.replaceChildren(...(tags.length ? [span(doc, "te-l", "Edit tags"), span(doc, "te-s", "Edit")] : [span(doc, "te-l", "Add tags")]));
  } else b?.remove();
  fitTitleTags(box);
}
