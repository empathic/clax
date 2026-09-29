import { describe, expect, it, vi } from "vitest";
import { whenParsed } from "../src/parsed";

describe("whenParsed", () => {
  it("runs now when the document has parsed", () => {
    const f = vi.fn();
    whenParsed(document, f);
    expect(f).toHaveBeenCalledTimes(1);
  });

  it("waits for DOMContentLoaded while the document is loading, and runs once", () => {
    const doc = document.implementation.createHTMLDocument("");
    Object.defineProperty(doc, "readyState", { value: "loading", configurable: true });
    const f = vi.fn();
    whenParsed(doc, f);
    expect(f).not.toHaveBeenCalled();
    doc.dispatchEvent(new Event("DOMContentLoaded"));
    doc.dispatchEvent(new Event("DOMContentLoaded"));
    expect(f).toHaveBeenCalledTimes(1);
  });
});
