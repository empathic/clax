import { afterEach, describe, expect, it } from "vitest";
import { setBadge } from "../q/badge";
import { setTabTitle, tabTitle } from "./tab-title";

afterEach(() => { document.title = ""; });

describe("tabTitle", () => {
  it("names the artifact before Clax, as the other pages name themselves", () => {
    expect(tabTitle("Q3 plan")).toBe("Q3 plan · Clax");
    expect(tabTitle(null)).toBe("Clax");
    expect(tabTitle("")).toBe("Clax");
  });
});

describe("setTabTitle", () => {
  it("keeps the unread count's prefix, and the badge keeps the title", () => {
    document.title = "(143) Clax";
    setTabTitle(document, "Q3 plan");
    expect(document.title).toBe("(143) Q3 plan · Clax");
    setTabTitle(document, "Renamed");
    expect(document.title).toBe("(143) Renamed · Clax");
    setBadge(document, 2);
    expect(document.title).toBe("(2) Renamed · Clax");
    setBadge(document, 0);
    expect(document.title).toBe("Renamed · Clax");
    setTabTitle(document, null);
    expect(document.title).toBe("Clax");
  });
});
