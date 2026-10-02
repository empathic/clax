import { afterEach, describe, expect, it } from "vitest";
import { THEME_KEY, applyChoice, flip, readChoice, shownScheme } from "./theme-model";

afterEach(() => { localStorage.clear(); delete document.documentElement.dataset.theme; });

describe("theme-model", () => {
  it("follows the system until flipped, and flipping back to the system's scheme follows it again", () => {
    expect(shownScheme(null, "dark")).toBe("dark");
    expect(flip(null, "dark")).toBe("light");
    expect(flip("light", "dark")).toBeNull();
    expect(flip(null, "light")).toBe("dark");
    expect(flip("dark", "light")).toBeNull();
  });
  it("stores and applies a choice, and clears both for null", () => {
    applyChoice("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(readChoice()).toBe("dark");
    applyChoice(null);
    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(localStorage.getItem(THEME_KEY)).toBeNull();
    localStorage.setItem(THEME_KEY, "sepia");
    expect(readChoice()).toBeNull();
  });
});
