import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../bridge/test/trusted";
import { settle, stubFetch } from "./people/fixtures";
import { flush, mount } from "./test/svelte";
import NewArtifactDialog from "./ui/NewArtifactDialog.svelte";
import { connectPrompt, formProblem, MAX_TITLE, pickGroups, type ListedAgent } from "./view/new-artifact";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement | HTMLTextAreaElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const submit = (root: Element) => flush(() => { root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); document.body.replaceChildren(); });

describe("formProblem", () => {
  it("asks for a one-line title and a brief within the daemon's limits", () => {
    expect(formProblem(" ", "x")).toBe("Give it a title.");
    expect(formProblem("a\nb", "x")).toBe("A title is one line of plain text.");
    expect(formProblem("a‮b", "x")).toBe("A title is one line of plain text.");
    expect(formProblem("é".repeat(MAX_TITLE + 1), "x")).toMatch(/at most 200/);
    expect(formProblem("é".repeat(MAX_TITLE), "x")).toBeNull();
    expect(formProblem("Plan", " \n")).toBe("Say what you want built.");
    expect(formProblem("Plan", "x".repeat(10_001))).toMatch(/at most 10,000/);
    expect(formProblem("Plan", "A chart")).toBeNull();
  });
});

describe("pickGroups", () => {
  const agents: ListedAgent[] = [
    { handle: null, name: "sam-claude", person: "p_sam", live: true },
    { handle: "a_mine", name: "clax", person: null, live: true, harness: "claude" },
    { handle: "a_gone", name: "old", person: null, live: false },
    { handle: null, name: null, person: "p_kim", live: true },
    { handle: "a_cx", name: null, person: null, live: true, harness: "codex" },
  ];
  it("puts the owner's own agents first by handle, and another person's by name under their name", () => {
    expect(pickGroups(agents, { public_id: "p_owner", owner: true }, new Map([["p_sam", "Sam"]]))).toEqual([
      { label: "Your agents", mine: true, agents: [{ to: "a_mine", name: "clax", who: null }, { to: "a_cx", name: "codex", who: null }] },
      { label: "Sam's agents", mine: false, agents: [{ to: "sam-claude", name: "sam-claude", who: "Sam's agent" }] },
    ]);
  });
  it("for a connected person, their own agents first, then the owner's", () => {
    const mine = [{ handle: "a_sam", name: "sam-claude", person: "p_sam", live: true }, ...agents.slice(1, 2)];
    expect(pickGroups(mine, { public_id: "p_sam" })).toEqual([
      { label: "Your agents", mine: true, agents: [{ to: "a_sam", name: "sam-claude", who: null }] },
      { label: "The owner's agents", mine: false, agents: [{ to: "a_mine", name: "clax", who: "The owner's agent" }] },
    ]);
  });
  it("offers nothing when no agent is live", () => {
    expect(pickGroups([{ handle: "a", name: "x", person: null, live: false }], { owner: true })).toEqual([]);
  });
});

describe("connectPrompt", () => {
  it("names the artifact's address and tells the agent to watch it and act on its brief", () => {
    const p = connectPrompt("http://localhost:7480/a/abc", "Q3 chart");
    expect(p).toContain("http://localhost:7480/a/abc");
    expect(p).toContain("\"Q3 chart\"");
    expect(p).toMatch(/watch/);
    expect(p).toMatch(/brief/);
  });
});

const started = { artifact: { id: "abc", title: "Q3 chart" }, url: "/a/abc", thread: { id: "t1" } };

describe("NewArtifactDialog", () => {
  it("creates from a title and a brief, lists the live agents and sends the brief to the one picked, then opens the artifact", async () => {
    const calls = stubFetch({
      "POST /api/artifacts:new": [201, started],
      "GET /api/agents": { agents: [{ handle: "a_mine", name: "clax", person: null, live: true }] },
      "POST /api/artifacts/abc/threads/t1/send": { thread: { id: "t1", feedback_state: { state: "sent" } } },
    });
    const assign = vi.fn();
    (await import("./nav")).nav.assign = assign;
    const m = mount(NewArtifactDialog, { me: { public_id: "p_owner", owner: true }, onClose: vi.fn() });
    // Nothing is sent without a brief.
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    submit(m.root);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Say what you want built.");
    expect(calls.some(c => c.url === "/api/artifacts:new")).toBe(false);
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "  A bar chart by region ");
    submit(m.root);
    await settle();
    expect(calls.find(c => c.url === "/api/artifacts:new")!.body).toEqual({ title: "Q3 chart", brief: "A bar chart by region" });
    expect(m.root.querySelector("h2")!.textContent).toBe("Who builds it?");
    const pick = m.root.querySelector<HTMLButtonElement>("button.agent")!;
    expect(pick.textContent).toContain("clax");
    click(pick);
    await settle();
    expect(calls.at(-1)).toEqual({ url: "/api/artifacts/abc/threads/t1/send", method: "POST", body: { to: "a_mine" } });
    expect(assign).toHaveBeenCalledWith("/a/abc");
  });

  it("with no live agent, shows the prompt to paste into one and opens the artifact on Close", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    stubFetch({ "POST /api/artifacts:new": [201, started], "GET /api/agents": { agents: [] } });
    const assign = vi.fn();
    (await import("./nav")).nav.assign = assign;
    const onClose = vi.fn();
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    submit(m.root);
    await settle();
    expect(m.root.textContent).toContain("No agent is connected.");
    const prompt = m.root.querySelector('pre[aria-label="Prompt"]')!.textContent!;
    expect(prompt).toContain(`${location.origin}/a/abc`);
    // An agent that connects is listed.
    stubFetch({ "GET /api/agents": { agents: [{ handle: "a_new", name: "late", person: null, live: true }] } });
    vi.advanceTimersByTime(3000);
    await settle();
    expect(m.root.querySelector("button.agent")!.textContent).toContain("late");
    // Closing after the artifact exists lands on it.
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Close")!);
    expect(assign).toHaveBeenCalledWith("/a/abc");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("shows the daemon's refusal and stays on the form", async () => {
    stubFetch({ "POST /api/artifacts:new": [403, { error: { code: "not_permitted", message: "your rights here do not let you create artifacts (clax-artifacts)" } }] });
    const m = mount(NewArtifactDialog, { me: { public_id: "p_sam" }, onClose: vi.fn() });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    submit(m.root);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("do not let you create");
    expect(m.root.querySelector("form")).not.toBeNull();
  });

  it("shows why a pick was refused and lets the person pick again", async () => {
    stubFetch({
      "POST /api/artifacts:new": [201, started],
      "GET /api/agents": { agents: [{ handle: null, name: "sam-claude", person: "p_sam", live: true }] },
      "GET /api/viewers?ids=p_sam": { viewers: [{ id: "p_sam", display_name: "Sam" }] },
      "POST /api/artifacts/abc/threads/t1/send": [400, { error: { code: "agent_cannot_read", message: "sam-claude may not read this artifact; nothing was sent" } }],
    });
    const assign = vi.fn();
    (await import("./nav")).nav.assign = assign;
    const m = mount(NewArtifactDialog, { me: { public_id: "p_owner", owner: true }, onClose: vi.fn() });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    submit(m.root);
    await settle();
    expect(m.root.querySelector("section h3")!.textContent).toBe("Sam's agents");
    click(m.root.querySelector("button.agent")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("may not read this artifact");
    expect(assign).not.toHaveBeenCalled();
    expect(m.root.querySelector("button.agent")!.getAttribute("aria-disabled")).toBe("false");
  });
});
