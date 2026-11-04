// The thread drawer's layout as Clax's own app dev setting: the DevTools
// panel's hook reads it from, and writes it to, the drawer's stored prefs.
import { beforeEach, describe, expect, it } from "vitest";
import { type DevtoolsHook, HOOK_KEY, registerDevSettings } from "../../../sdk/src/devtools";
import { DRAWER_KEY } from "../view/drawer-model";
import { drawer, registerDrawer } from "./drawer.svelte";

const hook = () => (window as unknown as Record<string, DevtoolsHook>)[HOOK_KEY];
const field = () => hook().snapshot().sections.find(s => s.id === "drawer")!.fields[0];

describe("the drawer's layout in the DevTools panel", () => {
  beforeEach(() => { localStorage.clear(); registerDrawer(registerDevSettings); hook().reset("drawer", "layout"); });

  it("is the default, docked right, until this browser keeps one", () => {
    expect(field()).toMatchObject({ value: "right", source: "site", applies: "live" });
    expect(field().options?.map(o => o.label)).toEqual(["Docked left", "Docked at the bottom", "Docked right", "Floating window"]);
  });

  it("sets the drawer from the panel, kept in the drawer's own prefs, and resets it", () => {
    expect(hook().set("drawer", "layout", "float")).toMatchObject({ ok: true, field: { value: "float", source: "you", inherited: { value: "right", source: "site" } } });
    expect(drawer.prefs.layout).toBe("float");
    expect(JSON.parse(localStorage.getItem(DRAWER_KEY)!).layout).toBe("float");
    expect(document.documentElement.dataset.drawer).toBe("float");
    expect(hook().reset("drawer", "layout")).toMatchObject({ ok: true, field: { value: "right", source: "site" } });
    expect(JSON.parse(localStorage.getItem(DRAWER_KEY)!).layout).toBeUndefined();
  });

  it("shows a change made in the drawer's own controls, the panel told by the rev", () => {
    const rev = hook().rev;
    drawer.set({ ...drawer.prefs, layout: "bottom" });
    expect(hook().rev).toBeGreaterThan(rev);
    expect(field()).toMatchObject({ value: "bottom", source: "you" });
  });
});
