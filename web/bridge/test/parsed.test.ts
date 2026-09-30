import { describe, expect, it, vi } from "vitest";
import { whenParsed } from "../src/parsed";

function loadingDoc() {
  const doc = document.implementation.createHTMLDocument("");
  const set = (state: DocumentReadyState) => {
    Object.defineProperty(doc, "readyState", { value: state, configurable: true });
    doc.dispatchEvent(new Event("readystatechange"));
  };
  Object.defineProperty(doc, "readyState", { value: "loading", configurable: true });
  return { doc, set };
}

describe("whenParsed", () => {
  it("runs now when the document has parsed", () => {
    const f = vi.fn();
    whenParsed(document, f);
    expect(f).toHaveBeenCalledTimes(1);
  });

  it("waits while the document is loading and runs once, when parsing ends", () => {
    const { doc, set } = loadingDoc();
    const f = vi.fn();
    whenParsed(doc, f);
    expect(f).not.toHaveBeenCalled();
    set("interactive");
    expect(f).toHaveBeenCalledTimes(1);
    doc.dispatchEvent(new Event("DOMContentLoaded"));
    set("complete");
    expect(f).toHaveBeenCalledTimes(1);
  });

  it("runs when an aborted parse (window.stop()) skips DOMContentLoaded", () => {
    // Aborting the parser moves readiness to "interactive" and "complete"
    // without firing DOMContentLoaded.
    const { doc, set } = loadingDoc();
    const f = vi.fn();
    whenParsed(doc, f);
    set("interactive");
    set("complete");
    expect(f).toHaveBeenCalledTimes(1);
  });
});
