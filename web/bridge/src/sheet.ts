// A stylesheet added to a document or a shadow root that a page's
// Content-Security-Policy does not refuse: a constructed sheet in the root's
// `adoptedStyleSheets` (CSSOM, which `style-src` without 'unsafe-inline'
// allows), else, where constructed sheets are missing, a <style> element.

/** A stylesheet in one root. */
export interface Sheet {
  /** Replaces the sheet's rules with `css`. */
  set(css: string): void;
  /** Takes the sheet out of its root. */
  remove(): void;
}

/** Adds a sheet holding `css` to `root`, after the root's other sheets. */
export function addSheet(root: Document | ShadowRoot, css: string): Sheet {
  const doc = root instanceof Document ? root : root.ownerDocument;
  const Ctor = doc.defaultView?.CSSStyleSheet;
  if (Ctor && "adoptedStyleSheets" in root && typeof Ctor.prototype.replaceSync === "function") {
    try {
      // A sheet is adopted only by roots of the document whose window made it.
      const sheet = new Ctor();
      sheet.replaceSync(css);
      root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
      return {
        set: text => sheet.replaceSync(text),
        remove: () => { root.adoptedStyleSheets = root.adoptedStyleSheets.filter(s => s !== sheet); },
      };
    } catch {
      // No constructable sheets after all: the <style> below.
    }
  }
  const style = doc.createElement("style");
  style.textContent = css;
  const parent = root instanceof Document ? (root.head ?? root.documentElement) : root;
  parent.appendChild(style);
  return { set: text => { style.textContent = text; }, remove: () => style.remove() };
}
