// The artifact's tags beside its by-line (docs/contract.md "Tags"): each a
// link to the gallery filtered by it, then, for a viewer who may set them
// (`may_tag`), Edit tags (Add tags while it carries none), which opens the
// tag editor (`ui/tag-editor.ts`, its own chunk). Plain DOM, as the by-line
// is, so the artifact entry carries no component for it.
import type { TagRef } from "../api";
import type { ViewState } from "./artifact-controller";

/** The artifact's tags and whether the viewer may set them: as last
 * refetched (`tags`), else as loaded. */
export const tagsOf = (s: ViewState): { tags: TagRef[]; may: boolean } => s.tags ?? { tags: s.data?.artifact.tags ?? [], may: !!s.data?.artifact.may_tag };

/** Keeps `ttl`'s tags (`.ttags`, made when first needed) showing `tags`,
 * with the edit button when `may`; `edit` gets the button when pressed.
 * Touches the DOM only when they change. */
export function setTitleTags(ttl: HTMLElement, tags: readonly TagRef[], may: boolean, edit: (b: HTMLButtonElement) => void): void {
  let box = ttl.querySelector<HTMLElement>(":scope > .ttags");
  if (!tags.length && !may) { box?.remove(); return; }
  const key = JSON.stringify([tags, may]);
  if (box?.dataset.k === key) return;
  const doc = ttl.ownerDocument;
  if (!box) {
    box = doc.createElement("span");
    box.className = "ttags";
    box.append(doc.createElement("span"));
    box.firstElementChild!.className = "tchips";
    ttl.append(box);
  }
  box.dataset.k = key;
  box.firstElementChild!.replaceChildren(...tags.map(t => {
    const a = doc.createElement("a");
    a.className = "tag";
    a.dataset.c = t.colour;
    a.href = `/?tag=${encodeURIComponent(t.name)}`;
    a.title = `The gallery, filtered by tag ${t.name}`;
    a.textContent = t.name;
    return a;
  }));
  // At phone width the chips give way to how many there are (theme.css
  // `.tn-n`): in the edit button for a viewer who may set them.
  const many = tags.length ? `${tags.length} tag${tags.length === 1 ? "" : "s"}` : "";
  const label = (what: string) => {
    const s = doc.createElement("span");
    s.className = "tn-w";
    s.textContent = what;
    const n = doc.createElement("span");
    n.className = "tn-n";
    n.textContent = many;
    return many ? [s, n] : [s];
  };
  box.querySelector(":scope > .tn-n")?.remove();
  let b = box.querySelector<HTMLButtonElement>(":scope > .tedit");
  if (!may) {
    b?.remove();
    box.append(...label("").slice(1));
    return;
  }
  if (!b) {
    const n = (b = doc.createElement("button"));
    n.type = "button";
    n.className = "tedit";
    n.setAttribute("aria-haspopup", "dialog");
    n.setAttribute("aria-expanded", "false");
    n.onclick = () => edit(n);
    box.append(n);
  }
  b.replaceChildren(...label(tags.length ? "Edit tags" : "Add tags"));
}
