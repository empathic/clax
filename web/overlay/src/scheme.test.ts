// The overlay takes the page's own light or dark (mount.ts followScheme), over the system's.
import { afterEach, describe, expect, it } from "vitest";
import { followScheme } from "./mount";

describe("followScheme", () => {
  afterEach(() => { document.documentElement.style.colorScheme = ""; });

  it("follows the page's chosen scheme as it switches, and leaves 'both' to the system", async () => {
    const host = document.createElement("div");
    document.documentElement.style.colorScheme = "dark";
    const stop = followScheme(document, host);
    expect(host.getAttribute("data-scheme")).toBe("dark");
    document.documentElement.style.colorScheme = "light";
    await Promise.resolve();
    expect(host.getAttribute("data-scheme")).toBe("light");
    document.documentElement.style.colorScheme = "light dark";
    await Promise.resolve();
    expect(host.hasAttribute("data-scheme")).toBe(false);
    stop();
  });
});
