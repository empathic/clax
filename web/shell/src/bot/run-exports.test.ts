import { describe, expect, it } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import RunExports from "./RunExports.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });

describe("RunExports", () => {
  it("exports a page of the run's state read-only, lists it and revokes it", async () => {
    const view = { pages: [{ path: "state.page" }], grants: [{ id: "g_1", name: "Laptop" }], exports: [], released: false };
    const exported = { ...view, exports: [{ id: "x1", grant: "g_1", actions: ["read"], expires_at: null, issued_at: "2026-10-21T09:00:00Z" }] };
    const calls = stubFetch({
      "GET /api/bot/runs/r1/exports": view,
      "POST /api/bot/runs/r1/exports": exported,
      "DELETE /api/bot/runs/r1/exports/x1": view,
    });
    const m = mount(RunExports, { run: "r1" });
    const details = m.root.querySelector<HTMLDetailsElement>("details.rexports")!;
    flush(() => { details.open = true; details.dispatchEvent(new Event("toggle")); });
    await settle();
    expect(m.root.textContent).toContain("its holder cannot use it yet");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Export")!);
    await settle();
    expect(calls.find(c => c.method === "POST")!.body).toEqual({ path: "state.page", grant: "g_1", actions: ["read"] });
    expect(m.root.querySelector('[aria-label="Your exported pages"] li span')!.textContent).toBe("Laptop: read");
    click(m.root.querySelector<HTMLElement>('button[aria-label="Revoke the page exported to Laptop"]')!);
    await settle();
    expect(calls.some(c => c.method === "DELETE" && c.url === "/api/bot/runs/r1/exports/x1")).toBe(true);
    expect(m.root.querySelector('[aria-label="Your exported pages"]')).toBeNull();
    m.unmount();
  });
});
