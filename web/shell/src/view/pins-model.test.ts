import { describe, expect, it } from "vitest";
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";
import { PIN_RIGHT_ROOM, pinPlaces } from "./pins-model";

const t = (id: string, file = "index.html", status: "open" | "resolved" = "open") => ({ id, status, anchor: { file } }) as unknown as Thread;
const at = (x: number, y: number, w = 100, h = 20): AnchorResult => ({ id: "x", found: true, method: "selector", rect: { x, y, w, h } }) as AnchorResult;

describe("pinPlaces", () => {
  it("numbers attached open threads on the page and skips unmeasured and scrolled-away ones without renumbering", () => {
    const threads = [t("a"), t("b"), t("c"), t("d", "other.html"), t("e", "index.html", "resolved")];
    const places = pinPlaces(threads, { a: at(10, 50), b: { id: "b", found: true, method: "selector", rect: null } as AnchorResult, c: at(10, -40, 100, 30) }, "index.html", 0);
    expect(places.map(p => [p.thread.id, p.n])).toEqual([["a", 1]]);
    expect(places[0]).toMatchObject({ left: 98, top: 38 });
  });
  it("keeps a pin clear of the stage's right edge", () => {
    const [p] = pinPlaces([t("a")], { a: at(0, 50, 1000) }, "index.html", 800);
    expect(p.left).toBe(800 - PIN_RIGHT_ROOM);
  });
  it("draws no pins over a document that did not greet", () => {
    expect(pinPlaces([t("a")], { a: at(0, 50) }, null, 0)).toEqual([]);
  });
});
