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
    click(a.querySelector('button[aria-label="End clax-multi-person"]')!);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/agents/h1", method: "DELETE", body: undefined });
    expect(onChanged).toHaveBeenCalledTimes(3);
  });

  it("revokes a person after asking, and shows the daemon's refusal as it is", async () => {
    const calls = stubFetch({ "DELETE /api/people/p1": [400, { error: { code: "not_a_person", message: "p1 is not a person" } }] });
    const ask = vi.fn(() => true);
    vi.stubGlobal("confirm", ask);
    const m = mount(PersonCard, { view: "owner", who: person(), onChanged: vi.fn() });
    // The name is the button's accessible name; its text is short, so a long name cannot widen the card.
    const revoke = m.root.querySelector<HTMLButtonElement>('button[aria-label="Revoke Sam"]')!;
    expect(revoke.textContent).toBe("Revoke");
    click(revoke);
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
  it("raises a Collaborator's agent to publish versions of one artifact, as section 3.4 does", async () => {
    const calls = stubFetch({ "PUT /api/agents/h1/elevate": { agent: {} } });
    const needArtifacts = vi.fn();
    const artifacts = [{ id: "01QR", title: "Quarterly Review" }, { id: "01XX", title: "<b>Other</b>" }];
    const m = mount(PersonCard, { view: "owner", who: person(), artifacts, needArtifacts, onChanged: vi.fn() });
    const panel = m.root.querySelector<HTMLDetailsElement>(".raise")!;
    flush(() => { panel.open = true; panel.dispatchEvent(new Event("toggle")); });
    expect(needArtifacts).toHaveBeenCalled();
    // At every artifact, a Collaborator's agent may publish versions of its person's own artifacts only: not held.
    expect(byLabel(panel, "Publish new versions")!.disabled).toBe(false);
    const at = panel.querySelector<HTMLSelectElement>(".where select")!;
    expect([...at.options].map(o => o.textContent)).toEqual(["Every artifact", "Quarterly Review", "<b>Other</b>"]);
    expect(panel.querySelector("b")).toBeNull();
    at.value = "01QR";
    change(at);
    const publish = byLabel(panel, "Publish new versions")!;
    expect(publish.checked).toBe(false);
    publish.checked = true;
    change(publish);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/agents/h1/elevate", method: "PUT", body: { access: [{ type: "clax-artifacts", actions: ["update"], locations: ["01QR"] }] } });
    // Raised, the panel names it by the artifact's title.
    m.update({ view: "owner", who: person({ agents: [agent({ elevate: [{ type: "clax-artifacts", actions: ["update"], locations: ["01QR"] }] })] }), artifacts, needArtifacts, onChanged: vi.fn() });
    expect(m.root.querySelector(".raise summary")!.textContent).toContain("(raised)");
    expect(m.root.querySelector(".raises")!.textContent).toBe("Publish new versions on “Quarterly Review”");
  });

  it("shows a person what the owner raised their agent with", () => {
    const reader = PRESETS["clax:reader"];
    const raised = agent({ rights: reader, elevate: [{ type: "clax-comments", actions: ["write"], locations: ["*"] }, { type: "clax-artifacts", actions: ["update"], locations: ["01QR"] }] });
    const m = mount(PersonCard, { view: "self", who: { public_id: "p1", display_name: "Lee", revoked_at: null, rights: reader, grants: [grant()], agents: [raised] }, artifacts: [{ id: "01QR", title: "Quarterly Review" }], onChanged: vi.fn() });
    const a = m.root.querySelector(".agent .narrow")!;
    const comment = [...a.querySelectorAll("label")].find(l => l.textContent!.trim().startsWith("Comment"))!;
    expect(comment.querySelector("input")!.checked).toBe(true);
    expect(comment.querySelector("input")!.disabled).toBe(true);
    expect(comment.textContent).toContain("Raised by the owner until this session ends");
    const publish = [...a.querySelectorAll("label")].find(l => l.textContent!.trim().startsWith("Publish new versions"))!;
    expect(publish.querySelector("input")!.checked).toBe(true);
    expect(publish.textContent).toContain("Raised by the owner on “Quarterly Review” until this session ends");
  });

  it("names an approval's page when the daemon names it", () => {
    const ap = (scope: "artifact" | "thread", id: string) => ({ id, approver: null, author: { public_id: "p1", name: "Sam" }, scope, artifact_id: "01QR", artifact_title: "Quarterly Review", thread_id: scope === "thread" ? "t1" : null, created_at: "", created_via: "inbox", request_id: null });
    const m = mount(PersonCard, { view: "owner", who: person(), approvals: [ap("artifact", "a1"), ap("thread", "a2")], onChanged: vi.fn() });
    const rows = [...m.root.querySelectorAll(".approvals .line span:first-child")].map(s => s.textContent);
    expect(rows).toEqual(["Allowed on “Quarterly Review”", "Allowed in one thread on “Quarterly Review”"]);
    expect(m.root.querySelector('.approvals button[aria-label="Always allow Sam"]')!.textContent).toBe("Always allow");
  });

  it("keeps the keyboard's focus on a control while its change is in flight, and moves it to a heading when the control goes", async () => {
    let answer: (v: Response) => void = () => {};
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(r => { answer = r; })));
    vi.stubGlobal("confirm", vi.fn(() => true));
    const m = mount(PersonCard, { view: "owner", who: person(), onChanged: vi.fn() });
    const panel = m.root.querySelector<HTMLDetailsElement>(".raise")!;
    flush(() => { panel.open = true; });
    const publish = byLabel(panel, "Publish new versions")!;
    publish.focus();
    publish.checked = true;
    change(publish);
    expect(m.root.querySelector("article")!.getAttribute("aria-busy")).toBe("true");
    expect(publish.disabled).toBe(false);
    expect(document.activeElement).toBe(publish);
    // A second change while the first is in flight is ignored and put back.
    const del = byLabel(panel, "Delete artifacts")!;
    del.checked = true;
    change(del);
    expect(del.checked).toBe(false);
    answer(new Response("{}"));
    await settle();
    expect(document.activeElement).toBe(publish);
    // Ending the agent: its row goes, so the focus moves to the Agents heading.
    const end = m.root.querySelector<HTMLButtonElement>('button[aria-label="End clax-multi-person"]')!;
    end.focus();
    click(end);
    answer(new Response("{}"));
    await settle();
    expect(document.activeElement!.textContent).toBe("Agents");
  });

  it("focuses the new name on Rename, and Rename again on Save or Cancel", async () => {
    stubFetch({ "PUT /api/agents/h1": { agent: {} } });
    const m = mount(PersonCard, { view: "self", who: { public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [agent()] }, onChanged: vi.fn() });
    const rename = () => m.root.querySelector<HTMLButtonElement>('button[aria-label="Rename clax-multi-person"]');
    click(rename()!);
    await settle();
    const input = m.root.querySelector<HTMLInputElement>(".rename input")!;
    expect(document.activeElement).toBe(input);
    click([...m.root.querySelectorAll(".rename button")].find(b => b.textContent === "Cancel")!);
    await settle();
    expect(document.activeElement).toBe(rename());
    click(rename()!);
    await settle();
    flush(() => { const i = m.root.querySelector<HTMLInputElement>(".rename input")!; i.value = "sam-notes"; i.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => { m.root.querySelector(".rename")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await settle();
    expect(m.root.querySelector(".rename")).toBeNull();
    expect(document.activeElement).toBe(rename());
  });

  it("shows the person's agents raised by an artifact's creator, by name and place, which only the creator removes", () => {
    const notes = [{ type: "clax-artifacts", actions: ["update"], locations: ["n1"] }];
    const raised = agent({ elevations: [{ granter: { public_id: "p2", name: "Mia", owner: false }, access: notes, created_at: "x" }] });
    const art = [{ id: "n1", title: "Mia's Notes" }];
    // The person's own view: who raised what, where.
    const self = mount(PersonCard, { view: "self", who: { public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [raised] }, artifacts: art, onChanged: vi.fn() });
    const publish = [...self.root.querySelectorAll(".agent label")].find(l => l.textContent!.trim().startsWith("Publish new versions"))!;
    expect(publish.textContent).toContain("Raised by Mia on “Mia's Notes” until this session ends");
    self.unmount();
    // The owner's view: read-only, beside the owner's own raise panel.
    const owner = mount(PersonCard, { view: "owner", who: person({ agents: [raised] }), artifacts: art, onChanged: vi.fn() });
    const by = owner.root.querySelector(".agent .note.by")!;
    expect(by.textContent).toBe("Raised by Mia until this session ends: Publish new versions on “Mia's Notes”");
    expect(by.querySelector("button, input")).toBeNull();
    expect(owner.root.querySelector(".raise summary")!.textContent).toBe("Raise this agent");
  });

  it("shows the owner the person's approvals of the owner's content, read-only, linking to the page or thread", () => {
    const ap = (o: object) => ({ id: "ap1", approver: "p1", author: { public_id: "o1", name: "Alex", owner: true }, scope: "artifact", artifact_id: "a1", artifact_title: "Quarterly Review", thread_id: null, created_at: "x", created_via: "shell", request_id: null, ...o });
    const m = mount(PersonCard, { view: "owner", who: person(), ofYours: [ap({}), ap({ id: "ap2", scope: "thread", thread_id: "t1" })] as never, onChanged: vi.fn() });
    const sec = m.root.querySelector(".approvals")!;
    const head = [...sec.querySelectorAll("h4")].find(h => h.textContent === "Approvals of your content")!;
    expect(head).toBeTruthy();
    const lines = [...sec.querySelectorAll(".line.yours")];
    expect(lines.map(l => l.textContent)).toEqual([
      "Allowed on “Quarterly Review” · Open the page", "Allowed in one thread on “Quarterly Review” · Open the thread",
    ]);
    expect(lines[1].querySelector("a")!.getAttribute("href")).toBe("/a/a1?thread=t1");
    expect(sec.querySelectorAll(".line.yours button").length).toBe(0);
    m.unmount();
    const none = mount(PersonCard, { view: "owner", who: person(), ofYours: [], onChanged: vi.fn() });
    expect(none.root.querySelector(".approvals")!.textContent).toContain("Not yet: their agents see your content once they allow it.");
    // The person's own view has no such section.
    const self = mount(PersonCard, { view: "self", who: { public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [] }, onChanged: vi.fn() });
    expect(self.root.textContent).not.toContain("Approvals of your content");
  });
});
