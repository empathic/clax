import { afterEach, describe, expect, it, vi } from "vitest";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import QueuePolicy from "./QueuePolicy.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Settings, Report queues", () => {
  it("shows whether members make report queues and turns it off", async () => {
    const calls = stubFetch({
      "GET /api/admin/queues": { members_create: true, source: "default" },
      "PUT /api/admin/queues": { members_create: false, source: "db" },
    });
    const m = mount(QueuePolicy, {});
    await settle();
    const sw = m.root.querySelector<HTMLButtonElement>("[role=switch]")!;
    expect(sw.getAttribute("aria-checked")).toBe("true");
    expect(m.root.querySelector("#rq-d")!.textContent).toContain("this Clax's default");
    flush(() => sw.click());
    await settle();
    expect(calls.find(c => c.method === "PUT")).toMatchObject({ url: "/api/admin/queues", body: { members_create: false } });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    expect(m.root.querySelector("#rq-d")!.textContent).toContain("only the owner and administrators make queues");
  });

  it("says why the setting could not load", async () => {
    stubFetch({ "GET /api/admin/queues": [403, { error: { code: "forbidden", message: "only the owner and admins use the admin pages" } }] });
    const m = mount(QueuePolicy, {});
    await settle();
    expect(m.root.querySelector("[role=alert]")!.textContent).toContain("Could not load the setting");
    expect(m.root.querySelector("[role=switch]")).toBeNull();
  });
});
