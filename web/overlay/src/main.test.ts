import { flushSync } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { type OverlayHandle, type OverlayOptions, overlay } from "./main";

const opts: OverlayOptions = { instance: "https://clax.example/", queue: "q1", auth: async () => { throw new Error("no backend"); } };
const shadow = () => document.querySelector(OVERLAY_TAG)?.shadowRoot ?? null;

let handle: OverlayHandle | null = null;
const start = (o: OverlayOptions) => { handle = overlay(o); flushSync(); return handle; };
afterEach(() => { handle?.close(); handle = null; });

describe("Clax.overlay", () => {
  it("refuses a launcher that is not a boolean, before it adds anything to the page", () => {
    expect(() => overlay({ ...opts, launcher: "no" as never })).toThrow(new TypeError("Clax.overlay: launcher must be a boolean"));
    expect(document.querySelector(OVERLAY_TAG)).toBeNull();
  });

  it("draws the launcher unless launcher is false", () => {
    start(opts);
    expect(shadow()?.querySelector(".launcher")).not.toBeNull();
    handle!.close();
    start({ ...opts, launcher: false });
    expect(shadow()?.querySelector(".cx")).not.toBeNull();
    expect(shadow()?.querySelector(".launcher")).toBeNull();
  });

  it("open() starts comment mode, and after close() the handle does nothing", () => {
    const h = start({ ...opts, launcher: false });
    h.open();
    flushSync();
    expect(shadow()?.querySelector(".hint")).not.toBeNull();
    h.close();
    expect(document.querySelector(OVERLAY_TAG)).toBeNull();
    h.open();
    h.toggleReports();
    h.close();
    expect(document.querySelector(OVERLAY_TAG)).toBeNull();
  });
});
