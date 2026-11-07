import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import KeyField from "./KeyField.svelte";

const SET = { set: true, last4: "abcd", set_at: null, set_by: null, last_ok_at: null };
const clear = (root: Element) => flush(() => [...root.querySelectorAll("button")].find(b => b.textContent?.startsWith("Clear"))!.click());

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("KeyField", () => {
  it("says a key the daemon reads on each call stops at once", () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const m = mount(KeyField, { info: SET, label: "Instance API key", onSave: async () => {}, onClear: async () => {} });
    clear(m.root);
    expect(asked).toHaveBeenCalledWith("Clear the instance api key? ClaxBot stops using it at once.");
  });

  it("says what clearing does where the caller knows better", () => {
    const asked = vi.fn(() => false);
    vi.stubGlobal("confirm", asked);
    const m = mount(KeyField, { info: SET, label: "Write token", clearSays: "New runs stop using it.", onSave: async () => {}, onClear: async () => {} });
    clear(m.root);
    expect(asked).toHaveBeenCalledWith("Clear the write token? New runs stop using it.");
  });
});
