import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "./fixtures";
import UnconnectedSwitch from "./UnconnectedSwitch.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("UnconnectedSwitch", () => {
  it("says what it lets through, revokes the owner's approval to turn off and makes one to turn on", async () => {
    const calls = stubFetch({ "DELETE /api/approvals/ap1": { approval: {} } });
    const onChanged = vi.fn();
    const m = mount(UnconnectedSwitch, { sw: { on: true, approval_id: "ap1" }, onChanged });
    expect(m.root.textContent).toContain("Comments from people on your LAN who are not connected reach your agents without asking. This includes connected people commenting from a browser or script that is not signed in as them.");
    const sw = m.root.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(sw.getAttribute("aria-checked")).toBe("true");
    click(sw);
    await settle();
    expect(calls).toEqual([{ url: "/api/approvals/ap1", method: "DELETE", body: undefined }]);
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("shows the daemon's refusal and stays as it was", async () => {
    stubFetch({ "POST /api/approvals": [403, { error: { code: "browser_required", message: "approving is done in a browser" } }] });
    const m = mount(UnconnectedSwitch, { sw: { on: false, approval_id: null }, onChanged: vi.fn() });
    click(m.root.querySelector('[role="switch"]')!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("approving is done in a browser");
    expect(m.root.querySelector('[role="switch"]')!.getAttribute("aria-checked")).toBe("false");
  });
});
