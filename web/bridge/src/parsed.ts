/**
 * Runs `f` once `doc` has finished parsing: now when it has, else on
 * `DOMContentLoaded`. The bridge runs first in `<head>`, so a shell order that
 * reads the page's content (anchor resolution, scroll-to) can arrive before
 * `<body>` exists; such orders wait for the whole document.
 */
export function whenParsed(doc: Document, f: () => void): void {
  if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", () => f(), { once: true });
  else f();
}
