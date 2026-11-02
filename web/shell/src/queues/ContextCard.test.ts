import { afterEach, describe, expect, it } from "vitest";
import { mount } from "../test/svelte";
import ContextCard from "./ContextCard.svelte";
import { CONTEXT } from "./fixtures";

afterEach(() => { document.body.replaceChildren(); });
// Intl may space a time with a narrow no-break space.
const text = (el: Element | null) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();

describe("ContextCard", () => {
  it("shows release commit flags and both time zones", () => {
    const m = mount(ContextCard, { context: CONTEXT, zone: "UTC", locale: "en-US" });
    const rel = m.root.querySelector(".rel")!;
    expect(text(rel)).toContain("0.6.0");
    expect(text(rel)).toContain("prod");
    const commit = rel.querySelector<HTMLAnchorElement>("a.commit")!;
    expect(commit.href).toBe("https://github.com/empathic/clax/commit/f7e862ab0aa1bb2cc3dd");
    expect(commit.textContent).toBe("f7e862ab0aa1");
    expect(commit.rel).toBe("noopener noreferrer");
    expect(commit.title).toBe("f7e862ab0aa1bb2cc3dd on github.com");
    expect([...m.root.querySelectorAll(".flags li")].map(text)).toEqual(["claxbot", "new_editor: variant-b", "beta: off"]);
    expect(text(m.root.querySelector('[data-zone="reader"]'))).toBe("Oct 29, 2026, 7:12 AM UTC");
    expect(text(m.root.querySelector('[data-zone="reporter"]'))).toBe("Oct 29, 2026, 12:12 AM PDT for the reporter (America/Los_Angeles)");
    expect(text(m.root)).toContain("Chrome 131 on macOS");
    expect(text(m.root)).toContain("1440 × 900 @2x");
  });

  it("shows the commit as text when no repository is named", () => {
    const m = mount(ContextCard, { context: { ...CONTEXT, app: undefined }, zone: "UTC", locale: "en-US" });
    expect(m.root.querySelector("a.commit")).toBeNull();
    expect(m.root.querySelector("code.commit")!.textContent).toBe("f7e862ab0aa1");
  });

  it("shows where in the code the report points, linked at its commit when the repository is named", () => {
    const code = { file: "web/src/a b.ts", start: 12, end: 14, side: "RIGHT", sha: "abcdef1234567890" };
    const m = mount(ContextCard, { context: { ...CONTEXT, app: { repo: "https://github.com/empathic/clax", code } }, zone: "UTC", locale: "en-US" });
    const a = m.root.querySelector<HTMLAnchorElement>("a.code")!;
    expect(a.textContent).toBe("web/src/a b.ts, lines 12–14 at abcdef123456");
    expect(a.href).toBe("https://github.com/empathic/clax/blob/abcdef1234567890/web/src/a%20b.ts#L12-L14");
    expect(a.rel).toBe("noopener noreferrer");
    // The old side of a diff, or no repository: text, no link.
    const old = mount(ContextCard, { context: { ...CONTEXT, app: { repo: "https://github.com/empathic/clax", code: { ...code, side: "LEFT", start: 3, end: 3 } } }, zone: "UTC" });
    expect(old.root.querySelector("a.code")).toBeNull();
    expect(text(old.root)).toContain("web/src/a b.ts, line 3 (old side) at abcdef123456");
    // Not well formed: no row.
    const bad = mount(ContextCard, { context: { ...CONTEXT, app: { code: { file: "a.ts", start: 5, end: 2 } } }, zone: "UTC" });
    expect(text(bad.root)).not.toContain("Code");
  });

  it("says when a report came with no context", () => {
    const m = mount(ContextCard, {});
    expect(text(m.root)).toContain("The report came with no context.");
  });
});
