import { afterEach, describe, expect, it } from "vitest";
import { OVERLAY_HOSTS, OVERLAY_TAG } from "../../bridge/src/anchor";
import { overlay } from "./main";

const opts = { instance: "https://feedback.example", queue: "q1", auth: async () => { throw new Error("no backend"); } };

afterEach(() => { for (const el of document.querySelectorAll(OVERLAY_HOSTS)) el.remove(); });

/** Every element name, attribute name and value, and (style sheets aside)
 * text in `root`, its open shadow roots included. */
function pageStrings(root: Document | ShadowRoot): string[] {
  const out: string[] = [];
  for (const el of root.querySelectorAll("*")) {
    out.push(el.localName);
    for (const a of el.attributes) out.push(a.name, a.value);
    for (const n of el.childNodes) if (n.nodeType === Node.TEXT_NODE && el.localName !== "style") out.push(n.textContent ?? "");
    if (el.shadowRoot) out.push(...pageStrings(el.shadowRoot));
  }
  return out;
}

describe("overlay()", () => {
  // As a React app in StrictMode does in development: its effect's setup,
  // cleanup and setup again, in one task.
  it("leaves nothing in the page when closed in the task that started it", () => {
    overlay(opts).close();
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  it("leaves nothing in the page when closed after its effects ran", async () => {
    const handle = overlay(opts);
    await new Promise(r => setTimeout(r, 0));
    expect(document.querySelectorAll(OVERLAY_TAG).length).toBeGreaterThan(0);
    handle.close();
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  // White label (Addendum 11): what the overlay adds to a host page (its
  // host elements, their attributes, their shadow trees) never says Clax.
  it("adds nothing to the page that names Clax: host element names, attributes, shadow trees", async () => {
    const handle = overlay(opts);
    try {
      await new Promise(r => setTimeout(r, 0));
      expect(document.querySelectorAll(OVERLAY_TAG).length).toBeGreaterThan(0);
      expect(document.querySelectorAll("clax-overlay")).toHaveLength(0);
      const seen = pageStrings(document);
      expect(seen).toContain(OVERLAY_TAG);
      expect(seen.filter(s => /clax/i.test(s))).toEqual([]);
    } finally { handle.close(); }
  });

  // The reporter lifecycle, reachable without the launcher (#34's point: a
  // host with `launcher: false` reads `waiting` from here for its own
  // button). `opts.auth` always refuses, so this is also the runtime
  // (not just dts-check's type-level) proof of the badge/failed fix:
  // with "Your reports" never opened (`reports` defaults to "button", so
  // it is, but no test here opens its panel), only the badge watch ever
  // calls the backend, and its failure alone must still reach auth: "error".
  it("handle.subscribe hears the current state at once, then auth going to error against a backend that only ever refuses, and nothing more after close", async () => {
    const seen: { auth: string }[] = [];
    const handle = overlay(opts);
    const unsubscribe = handle.subscribe(s => seen.push(s));
    try {
      expect(seen).toHaveLength(1);
      // Already "loading", not "idle": the badge watch starts synchronously
      // while the overlay mounts (`reports` defaults to "button", so it is on).
      expect(seen[0]).toMatchObject({ auth: "loading", waiting: null, reports: null });
      for (let i = 0; i < 20 && seen.at(-1)?.auth !== "error"; i++) await new Promise(r => setTimeout(r, 0));
      expect(seen.at(-1)).toMatchObject({ auth: "error" });
      const before = seen.length;
      handle.close();
      await new Promise(r => setTimeout(r, 0));
      expect(seen).toHaveLength(before);
    } finally { unsubscribe(); handle.close(); }
  });
});
