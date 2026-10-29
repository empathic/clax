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

  it("says when a report came with no context", () => {
    const m = mount(ContextCard, {});
    expect(text(m.root)).toContain("The report came with no context.");
  });
});
