import { describe, expect, it, vi } from "vitest";
import Probe from "./Probe.svelte";
import { flush, mount } from "./svelte";

describe("the Svelte test helper", () => {
  it("mounts, applies a click and new props, and unmounts with the component's teardown", () => {
    const gone = vi.fn();
    const view = mount(Probe, { label: "n", onGone: gone });
    const button = view.root.querySelector("button")!;
    expect(button.textContent).toBe("n 0");
    flush(() => button.click());
    expect(button.textContent).toBe("n 1");
    view.update({ label: "m", onGone: gone });
    expect(view.root.querySelector("button")!.textContent).toBe("m 1");
    view.unmount();
    expect(view.root.querySelector("button")).toBeNull();
    expect(gone).toHaveBeenCalledTimes(1);
  });
});
