// The artifact's title in the top bar and the tab, once the view is live
// (docs/contract.md "Title on create", "Renaming"). The stream's chunk loads
// it on the stream's first `ready`, so the artifact entry carries none of it:
// the daemon's page already names the artifact in the tab and the heading.
//
// The tab reads "<title> · Clax" (`tab-title.ts`) and follows the title as
// it changes: a rename here or in another view (`artifact_meta`), or a
// refetch. For a viewer who may update the artifact (`may_edit`), the
// heading's text becomes a button, "Rename" its description: pressed, a text
// field takes the heading's place. Enter or leaving it saves, Escape
// cancels, and an empty field saves the artifact as untitled (its page's
// `<title>`, else "Untitled", shows). Plain DOM with its own styles, as the
// title's other parts are.
import { ApiError, type Artifact, getToken } from "../api";
import type { ArtifactController, ViewState } from "./artifact-controller";
import { setTabTitle } from "./tab-title";

/** The notice of a rename the daemon refused, before its reason. */
export const RENAME_FAILED = "The artifact was not renamed";

/** The text field's accessible name. */
export const FIELD_LABEL = "Artifact title";

const CSS = `.ttl h1 > .ttl-name { all: unset; display: inline-block; vertical-align: top; box-sizing: border-box; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: text; border-radius: var(--radius-xs, 4px); }
.ttl h1 > .ttl-name:hover { background: var(--hover); }
.ttl h1 > .ttl-name:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }
.ttl > .ttl-edit { flex: 0 0 100%; min-width: 0; box-sizing: border-box; margin: 0; padding: 0 4px; font: 600 15px/1.25 var(--font); color: var(--fg); background: var(--bg); border: 1px solid var(--border-hover); border-radius: var(--radius-xs, 4px); }
.ttl > .ttl-edit:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; border-color: transparent; }`;

/** What the heading shows in `s`: the title (null while no artifact is
 * shown), the artifact's own title (empty while it is untitled), and
 * whether this viewer may rename it here. */
export function titleOf(s: ViewState): { title: string | null; own: string; may: boolean } {
  const a = !s.error && s.data ? s.data.artifact : null;
  if (!a) return { title: null, own: "", may: false };
  return { title: a.title, own: a.untitled ? "" : a.title, may: !!a.may_edit && !s.deleted && !a.live };
}

/** Saves `value`, trimmed, as artifact `id`'s title (empty: untitled)
 * with `PATCH /api/artifacts/<id>`, and answers the artifact. The owner's
 * shell sends its token; anyone else's request is their browser's. */
export async function renameArtifact(id: string, value: string): Promise<Artifact> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  const token = await getToken();
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await fetch(`/api/artifacts/${encodeURIComponent(id)}`, { method: "PATCH", headers, body: JSON.stringify({ title: value.trim() }) });
  const body = await res.json().catch(() => null) as { artifact?: Artifact; error?: { message?: string; code?: string } } | null;
  if (!res.ok || !body?.artifact) throw new ApiError(res.status, body?.error?.message ?? (res.statusText || `HTTP ${res.status}`), body?.error?.code ?? null);
  return body.artifact;
}

/** Puts `a`'s title, whether it is untitled, description and icon into
 * the view's artifact. */
export function takeTitle(c: Pick<ArtifactController, "set">, a: Pick<Artifact, "title" | "untitled" | "description" | "icon">): void {
  c.set(s => (s.data ? { data: { ...s.data, artifact: { ...s.data.artifact, title: a.title, untitled: a.untitled, description: a.description, icon: a.icon } } } : {}));
}

/** Opens the text field in place of heading `h1`, holding `own` (the
 * artifact's own title; empty while untitled, `shown` as its placeholder).
 * Enter or leaving it calls `save` with what it holds when that changed,
 * Escape cancels; then the heading is back and `done` is called. */
export function editTitle(h1: HTMLElement, own: string, shown: string, save: (value: string) => void, done: () => void): HTMLInputElement {
  const doc = h1.ownerDocument;
  const input = doc.createElement("input");
  input.type = "text";
  input.className = "ttl-edit";
  input.maxLength = 200;
  input.autocomplete = "off";
  input.spellcheck = false;
  input.setAttribute("aria-label", FIELD_LABEL);
  input.value = own;
  input.placeholder = shown;
  let open = true;
  const close = (keep: boolean) => {
    if (!open) return;
    open = false;
    const value = input.value.trim();
    input.remove();
    h1.hidden = false;
    if (keep && value !== own) save(value);
    done();
  };
  input.addEventListener("keydown", e => {
    // The field's keys are its own, never the view's shortcuts.
    e.stopPropagation();
    if (e.key === "Enter") { e.preventDefault(); close(true); }
    else if (e.key === "Escape") { e.preventDefault(); close(false); }
  });
  input.addEventListener("blur", () => close(true));
  h1.hidden = true;
  h1.after(input);
  input.focus();
  input.select();
  return input;
}

/** Keeps `doc`'s tab on `c`'s title and, while this viewer may rename the
 * artifact, its heading a button that opens the text field. The result
 * stops it and leaves the heading as plain text. */
export function followTitle(c: ArtifactController, doc: Document = document): () => void {
  const h1 = doc.querySelector<HTMLElement>(".topbar .ttl > h1");
  if (!doc.getElementById("ttl-css")) {
    const style = doc.createElement("style");
    style.id = "ttl-css";
    style.textContent = CSS;
    doc.head.append(style);
  }
  let hint: HTMLElement | null = null;
  let editing = false;
  let focusBack = false;
  const button = (title: string): HTMLButtonElement => {
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "ttl-name";
    b.textContent = title;
    b.title = "Rename";
    if (!hint) {
      hint = doc.createElement("span");
      hint.id = "ttl-rename";
      hint.hidden = true;
      hint.textContent = "Rename";
      h1!.parentElement!.append(hint);
    }
    b.setAttribute("aria-describedby", hint.id);
    b.onclick = () => open();
    return b;
  };
  const open = () => {
    if (editing || !h1) return;
    const t = titleOf(c.state.get());
    if (!t.may || t.title === null) return;
    editing = true;
    editTitle(h1, t.own, t.title, value => {
      void renameArtifact(c.id, value).then(a => {
        // The button takes the new title first, so the view, finding the
        // heading's text already that, leaves the button (and its focus).
        const b = h1.firstElementChild;
        if (b instanceof HTMLButtonElement && h1.childNodes.length === 1) b.textContent = a.title;
        takeTitle(c, a);
      }, e => c.setNotice(`${RENAME_FAILED}: ${e instanceof ApiError ? e.message.replace(/^\d+ /, "") : "the daemon did not answer"}.`));
    }, () => { editing = false; focusBack = true; show(c.state.get()); });
  };
  const show = (s: ViewState) => {
    if (c.disposed) return;
    const t = titleOf(s);
    setTabTitle(doc, t.title);
    if (!h1 || editing || t.title === null) return;
    const b = h1.firstElementChild;
    if (t.may) {
      // The view writes the heading's text when the title changes; the
      // button goes back around it.
      if (!(b instanceof HTMLButtonElement) || b.textContent !== t.title || h1.childNodes.length !== 1) h1.replaceChildren(button(t.title));
      if (focusBack) (h1.firstElementChild as HTMLElement).focus();
    } else if (b) h1.textContent = t.title;
    focusBack = false;
  };
  const off = c.state.subscribe(show);
  return () => {
    off();
    if (h1?.firstElementChild) h1.textContent = h1.textContent;
    (hint as HTMLElement | null)?.remove();
  };
}
