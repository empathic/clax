// How a page opens the overlay without any code of its own: a click on its
// trigger (the install's selector, or an element the page marks
// `data-feedback`, the mark the loader reads too), and the `F` key.
//
// The click is matched when it happens, by one capturing listener on the
// document (so a page's stopPropagation does not hide it), and a button a
// client-side framework renders later, or renders again, is a trigger from its
// first frame, with nothing to watch for and nothing added to it. The element
// keeps what it is: a real button stays one, and Enter and Space on it are the
// click this hears. A page's own handler on it still runs.
//
// `F` is capital F, so the page's lowercase `f` stays the page's. It listens at
// the window's bubble phase, last: a key the page used (and marked used, as
// Changeling's chords do) is not `F` here. Never in a text field, however
// deep in a shadow root, or with a modifier other than Shift.

/** What the page marks an element with: `data-feedback="report"` or `"reports"`. */
export const MARK = "data-feedback";
const MARKED = `[${MARK}]`;

/** Whether keys pressed at `at` are typing: a field, or anything editable. */
export function isTyping(at: EventTarget | null | undefined): boolean {
  if (!(at instanceof HTMLElement)) return false;
  if (at.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at.tagName)) return true;
  return /^(textbox|searchbox|combobox)$/.test(at.getAttribute("role") ?? "");
}

export interface Entries {
  /** The install's trigger selector, if any. */
  selector: string | null;
  /** Whether `F` opens the overlay. */
  fKey: boolean;
  /** Opens the overlay; `opener` gets focus back when it closes. */
  open(opener: HTMLElement | null): void;
  /** Opens "Your reports". */
  reports(): void;
}

/** Whether `el` is reached by the keyboard and named as a control on its own. */
const interactive = (el: Element) => el.matches("button, a[href], input, select, summary, [role=button], [role=link], [role=menuitem], [tabindex]");

/** Starts listening on `doc`; the result stops. */
export function startEntries(doc: Document, e: Entries): () => void {
  const win = doc.defaultView ?? window;
  let selector = e.selector;
  if (selector) {
    try { doc.querySelector(selector); } catch {
      console.warn(`Clax.overlay: the trigger ${JSON.stringify(selector)} is not a CSS selector; ignoring it`);
      selector = null;
    }
  }
  const warned = new WeakSet<Element>();
  const onClick = (ev: MouseEvent) => {
    // The path, not the target alone: a trigger inside a page's own open shadow root counts.
    for (const n of ev.composedPath()) {
      if (!(n instanceof Element)) continue;
      const action = n.matches(MARKED) ? n.getAttribute(MARK) : selector && n.matches(selector) ? "report" : null;
      if (action !== "report" && action !== "reports") continue;
      // A trigger that is a link is left to the browser for a new tab or window.
      if (n.localName === "a" && (ev.metaKey || ev.ctrlKey || ev.shiftKey)) return;
      if (!interactive(n) && !warned.has(n)) {
        warned.add(n);
        console.warn("Clax.overlay: this trigger is not a button or link, so the keyboard and a screen reader cannot reach it", n);
      }
      ev.preventDefault();
      if (action === "reports") e.reports();
      else e.open(n instanceof HTMLElement ? n : null);
      return;
    }
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key !== "F" || ev.defaultPrevented || ev.metaKey || ev.ctrlKey || ev.altKey || ev.isComposing || ev.repeat) return;
    if (isTyping(ev.composedPath()[0])) return;
    ev.preventDefault();
    const at = doc.activeElement;
    e.open(at instanceof HTMLElement && at !== doc.body ? at : null);
  };
  doc.addEventListener("click", onClick, true);
  if (e.fKey) win.addEventListener("keydown", onKey);
  return () => {
    doc.removeEventListener("click", onClick, true);
    win.removeEventListener("keydown", onKey);
  };
}
