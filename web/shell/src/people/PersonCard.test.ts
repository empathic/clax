import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { agent, COLLAB, grant, person, settle, stubFetch } from "./fixtures";
import { MACHINE_BOUNDARY, PRESETS } from "./model";
import PersonCard from "./PersonCard.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const change = (el: HTMLInputElement | HTMLSelectElement) => flush(() => { el.dispatchEvent(new Event("change", { bubbles: true })); });
const HOSTILE = `<img src=x onerror="window.pwned=1">Sam`;
const byLabel = (root: Element, label: string) => [...root.querySelectorAll<HTMLInputElement>("label")].find(l => l.textContent!.trim() === label)?.querySelector("input") ?? null;

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("PersonCard", () => {
  it("shows a person's machines with their key fingerprints and their agents, by machine", () => {
    const p = person({ grants: [grant(), grant({ id: "g2", client: { name: "sam-air", key: "SHA256:77aa", peer_ip: "192.168.1.41" } })],
      agents: [agent(), agent({ handle: "h2", name: "web-fix", harness: "claude", grant_id: "g2" })] });
    const m = mount(PersonCard, { view: "owner", who: p, onChanged: vi.fn() });
    const machines = [...m.root.querySelectorAll(".machine")];
    expect(machines.map(x => x.querySelector(".mname")!.textContent)).toEqual(["sam-mbp", "sam-air"]);
    expect(machines[0].textContent).toContain("SHA256:9f2cAbCd");
    expect(machines[0].textContent).toContain("192.168.1.40");
    const agents = [...m.root.querySelectorAll(".agent")];
    expect(agents.map(a => a.querySelector(".aname")!.textContent)).toEqual(["clax-multi-person", "web-fix"]);
    expect(agents[1].textContent).toContain("claude");
    expect(agents[1].textContent).toContain("sam-air");
  });

  it("keeps hostile names as text, never markup", () => {
    const p = person({ display_name: HOSTILE, grants: [grant({ client: { name: HOSTILE, key: "SHA256:x", peer_ip: "1.2.3.4" } })], agents: [agent({ name: HOSTILE })] });
    const m = mount(PersonCard, { view: "owner", who: p, onChanged: vi.fn() });
    expect(m.root.querySelector("img")).toBeNull();
    expect(m.root.querySelector("h3")!.textContent).toContain(HOSTILE);
    expect(m.root.querySelector(".mname")!.textContent).toBe(HOSTILE);
    expect(m.root.querySelector(".aname")!.textContent).toBe(HOSTILE);
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  });

  it("lets the owner set a person's preset, raise one agent with the machine-boundary line beside it, and end it", async () => {
    const calls = stubFetch({ "PUT /api/people/p1/rights": { person: {} }, "PUT /api/agents/h1/elevate": { agent: {} }, "DELETE /api/agents/h1": { agent: {} } });
    vi.stubGlobal("confirm", vi.fn(() => true));
    const onChanged = vi.fn();
    const p = person({ rights: PRESETS["clax:reader"], agents: [agent({ rights: PRESETS["clax:reader"] })] });
    const m = mount(PersonCard, { view: "owner", who: p, onChanged });
    const preset = m.root.querySelector<HTMLSelectElement>("select.preset")!;
    expect(preset.value).toBe("clax:reader");
    preset.value = "clax:full";
    change(preset);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/people/p1/rights", method: "PUT", body: { rights: "clax:full" } });
    const a = m.root.querySelector(".agent")!;
    expect(a.textContent).toContain(MACHINE_BOUNDARY);
    // The person's own narrowing is theirs: the owner raises, never narrows.
    expect(a.querySelector(".narrow")).toBeNull();
    const publish = byLabel(a, "Publish new versions")!;
    expect(publish.checked).toBe(false);
    expect(byLabel(a, "Read artifacts")!.disabled).toBe(true);
    publish.checked = true;
    change(publish);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/agents/h1/elevate", method: "PUT", body: { access: [{ type: "clax-artifacts", actions: ["update"], locations: ["*"] }] } });
    click([...a.querySelectorAll("button")].find(b => b.textContent === "End")!);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/agents/h1", method: "DELETE", body: undefined });
    expect(onChanged).toHaveBeenCalledTimes(3);
  });

  it("revokes a person after asking, and shows the daemon's refusal as it is", async () => {
    const calls = stubFetch({ "DELETE /api/people/p1": [400, { error: { code: "not_a_person", message: "p1 is not a person" } }] });
    const ask = vi.fn(() => true);
    vi.stubGlobal("confirm", ask);
    const m = mount(PersonCard, { view: "owner", who: person(), onChanged: vi.fn() });
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Revoke Sam")!);
    await settle();
    expect(ask).toHaveBeenCalled();
    expect(calls.at(-1)).toEqual({ url: "/api/people/p1", method: "DELETE", body: undefined });
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("p1 is not a person");
  });

  it("puts a refused narrowing's checkbox back and says why", async () => {
    stubFetch({ "PUT /api/people/me/grants/g1/narrow": [400, { error: { code: "not_narrower", message: "that would widen this machine's rights" } }] });
    const m = mount(PersonCard, { view: "self", who: { public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [] }, onChanged: vi.fn() });
    const del = byLabel(m.root.querySelector(".machine")!, "Delete artifacts")!;
    del.checked = false;
    change(del);
    await settle();
    expect(del.checked).toBe(true);
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("that would widen this machine's rights");
  });

  it("lets a person narrow a machine and one agent of it, and gives this machine's agents no rights controls", async () => {
    const calls = stubFetch({ "PUT /api/people/me/grants/g1/narrow": { grant: {} }, "PUT /api/agents/h1/narrow": { agent: {} } });
    const me = { public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [agent()] };
    const m = mount(PersonCard, { view: "self", who: me, onChanged: vi.fn() });
    const machine = m.root.querySelector(".machine")!;
    const del = byLabel(machine, "Delete artifacts")!;
    expect(del.checked).toBe(true);
    del.checked = false;
    change(del);
    await settle();
    const sent = calls.at(-1)!;
    expect(sent.url).toBe("/api/people/me/grants/g1/narrow");
    expect(JSON.stringify(sent.body)).not.toContain("delete");
    const a = m.root.querySelector(".agent")!;
    expect(a.textContent).toContain(MACHINE_BOUNDARY);
    expect(a.querySelector(".raise")).toBeNull();
    const send = byLabel(a, "Send threads to agents")!;
    send.checked = false;
    change(send);
    await settle();
    expect(calls.at(-1)!.url).toBe("/api/agents/h1/narrow");
    m.unmount();
    // The owner's own card: an agent of this machine (no grant) has no controls, and a word on why.
    const own = mount(PersonCard, { view: "self", owner: true, who: { ...me, public_id: "o1", display_name: "Alex", rights: PRESETS["clax:full"], grants: [], agents: [agent({ person: null, grant_id: null, rights: null, name: "clax-claude" })] }, onChanged: vi.fn() });
    const local = own.root.querySelector(".agent")!;
    expect(local.querySelector('input[type="checkbox"]')).toBeNull();
    expect(local.textContent).toContain("this machine");
  });
});
