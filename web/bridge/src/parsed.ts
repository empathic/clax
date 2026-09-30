/**
 * Runs `f` once `doc` has finished parsing: now when it has, else on the first
 * `readystatechange` that leaves "loading". That change comes before
 * `DOMContentLoaded` (without waiting for deferred scripts) and also when the
 * parse is aborted (`window.stop()`), which skips `DOMContentLoaded`. The
 * bridge runs before the page's own content, so a shell order that reads it
 * (anchor resolution, scroll-to) can arrive before `<body>` exists; such orders
 * wait for the whole document.
 */
export function whenParsed(doc: Document, f: () => void): void {
  if (doc.readyState !== "loading") { f(); return; }
  const check = () => {
    if (doc.readyState === "loading") return;
    doc.removeEventListener("readystatechange", check);
    f();
  };
  doc.addEventListener("readystatechange", check);
}
