// The anchors entry, imported by its package name as the extension does
// (scripts/sdk-entries.mjs aliases it to its source here).
import { afterEach, describe, expect, it } from "vitest";
import { buildElementAnchor, buildRangeAnchor, INDEX_FILE, OVERLAY_TAG, resolveAnchor } from "@empathic/clax-sdk/anchor";
import * as bridgeAnchor from "../../bridge/src/anchor";

afterEach(() => { document.body.innerHTML = ""; });

describe("@empathic/clax-sdk/anchor", () => {
  it("is web/bridge's own code, not a copy", () => {
    expect(buildElementAnchor).toBe(bridgeAnchor.buildElementAnchor);
    expect(resolveAnchor).toBe(bridgeAnchor.resolveAnchor);
    expect(OVERLAY_TAG).toBe("clax-overlay");
  });

  it("resolves an element's anchor to the same element", () => {
    document.body.innerHTML = `<main><h1>Totals</h1><p id="t">The total is wrong</p></main>`;
    const el = document.getElementById("t")!;
    const anchor = buildElementAnchor(document, el);
    expect(anchor.file).toBe(INDEX_FILE);
    expect(resolveAnchor(document, anchor)?.element).toBe(el);
  });

  it("finds a quoted range again by its text after the page's structure changes", () => {
    document.body.innerHTML = `<p>Before. The total is wrong. After.</p>`;
    const text = document.querySelector("p")!.firstChild!;
    const range = document.createRange();
    range.setStart(text, 8);
    range.setEnd(text, 27);
    expect(range.toString()).toBe("The total is wrong.");
    const anchor = buildRangeAnchor(document, range);
    expect(anchor.kind).toBe("range");
    document.body.innerHTML = `<section><div><p>Before. The total is wrong. After.</p></div></section>`;
    expect(resolveAnchor(document, anchor)?.range?.toString()).toBe("The total is wrong.");
  });
});
