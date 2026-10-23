import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { meBot, week } from "./bot/fixtures";
import { forget } from "./bot/me";
import NewTile from "./ui/NewTile.svelte";
import { dispatchTrusted } from "../../bridge/test/trusted";
import { settle, stubFetch } from "./people/fixtures";
import { flush, mount } from "./test/svelte";
import NewArtifactDialog from "./ui/NewArtifactDialog.svelte";
import { forgetViewer } from "./ui/may-create";
import { connectPrompt, formProblem, MAX_TITLE, pickGroups, type ListedAgent } from "./view/new-artifact";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const type = (el: HTMLInputElement | HTMLTextAreaElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const submit = (root: Element) => flush(() => { root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });

beforeEach(() => { forget(); });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); document.body.replaceChildren(); });

describe("formProblem", () => {
  it("asks for a one-line title and a brief within the daemon's limits", () => {
    expect(formProblem(" ", "x")).toBe("Give it a title.");
    expect(formProblem("a\nb", "x")).toBe("A title is one line of plain text.");
    expect(formProblem("a‮b", "x")).toBe("A title is one line of plain text.");
    expect(formProblem("a\u200bb", "x")).toBe("A title is one line of plain text.");
    // Joiners and direction marks are text, as in any artifact title.
    expect(formProblem("Team \u{1f468}\u200d\u{1f469}", "x")).toBeNull();
    expect(formProblem("\u05e9\u05dc\u05d5\u05dd \u200fQ3", "x")).toBeNull();
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
      "GET /api/agents?artifact=abc": { agents: [{ handle: "a_mine", name: "clax", person: null, live: true }] },
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
    stubFetch({ "POST /api/artifacts:new": [201, started], "GET /api/agents?artifact=abc": { agents: [] } });
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
    stubFetch({ "GET /api/agents?artifact=abc": { agents: [{ handle: "a_new", name: "late", person: null, live: true }] } });
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
      "GET /api/agents?artifact=abc": { agents: [{ handle: null, name: "sam-claude", person: "p_sam", live: true }] },
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

  it("reads the list again when the agent picked has gone", async () => {
    let reads = 0;
    stubFetch({
      "POST /api/artifacts:new": [201, started],
      "GET /api/agents?artifact=abc": () => (++reads === 1
        ? { agents: [{ handle: "a_gone", name: "gone", person: null, live: true }, { handle: "a_here", name: "here", person: null, live: true }] }
        : { agents: [{ handle: "a_here", name: "here", person: null, live: true }] }),
      "POST /api/artifacts/abc/threads/t1/send": [400, { error: { code: "unknown_agent", message: "no live agent that may watch this artifact has that name or handle" } }],
    });
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    submit(m.root);
    await settle();
    expect(m.root.querySelectorAll("button.agent")).toHaveLength(2);
    click(m.root.querySelector("button.agent")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toContain("no live agent");
    expect([...m.root.querySelectorAll("button.agent")].map(b => b.querySelector(".nm")!.textContent)).toEqual(["here"]);
  });

  it("closed while Create is in flight, lands on the artifact made and polls nothing", async () => {
    let answer: (r: Response) => void = () => {};
    const calls: string[] = [];
    vi.stubGlobal("fetch", vi.fn((url: string) => {
      calls.push(url);
      if (url === "/api/artifacts:new") return new Promise<Response>(r => { answer = r; });
      return Promise.resolve(new Response("{}", { status: 404 }));
    }));
    const setInt = vi.spyOn(globalThis, "setInterval");
    const assign = vi.fn();
    (await import("./nav")).nav.assign = assign;
    const onClose = vi.fn();
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    submit(m.root);
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Cancel")!);
    expect(onClose).toHaveBeenCalled();
    m.unmount();
    answer(new Response(JSON.stringify(started), { status: 201 }));
    await settle();
    expect(assign).toHaveBeenCalledWith("/a/abc");
    expect(setInt).not.toHaveBeenCalled();
    expect(calls.some(u => u.startsWith("/api/agents"))).toBe(false);
    setInt.mockRestore();
  });

  it("retries Create with the same key after a lost answer, and a new one after a refusal", async () => {
    const keys: (string | null)[] = [];
    let n = 0;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if (url !== "/api/artifacts:new") return new Response("{}", { status: 404 });
      keys.push(new Headers(init?.headers).get("idempotency-key"));
      n++;
      if (n === 1) throw new TypeError("Failed to fetch");
      return new Response(JSON.stringify({ error: { code: "invalid_title", message: "bad title" } }), { status: 400 });
    }));
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Q3 chart");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "A bar chart");
    for (let i = 0; i < 3; i++) { submit(m.root); await settle(); }
    expect(keys).toHaveLength(3);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBe(keys[0]);
    expect(keys[2]).not.toBe(keys[1]);
  });
});

describe("the New dialog with ClaxBot", () => {
  beforeEach(() => { forget(); });
  const fill = (m: { root: HTMLElement }) => {
    type(m.root.querySelector<HTMLInputElement>("#na-name")!, "Grocery splitter");
    type(m.root.querySelector<HTMLTextAreaElement>("#na-brief")!, "Split a grocery receipt");
  };

  it("names its fields Name and Description, and with ClaxBot available builds at once and opens the run", async () => {
    const calls = stubFetch({ "GET /api/me/bot": meBot({ week: week({ spent_milli: 310_000 }) }), "POST /api/artifacts:new": [201, { ...started, run: { id: "r1" } }] });
    const assign = vi.fn();
    (await import("./nav")).nav.assign = assign;
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    await settle();
    expect([...m.root.querySelectorAll("label")].map(l => l.textContent)).toEqual(["Name", "Description"]);
    const sw = m.root.querySelector<HTMLElement>('[role="switch"][aria-label="ClaxBot builds it now"]')!;
    expect(sw.getAttribute("aria-checked")).toBe("true");
    expect(m.root.querySelector("#na-bot-l")!.textContent).toContain("You have 1,690 credits left this week.");
    expect(m.root.querySelector('button[type="submit"]')!.textContent).toBe("Create and build");
    fill(m);
    submit(m.root);
    await settle();
    expect(calls.find(c => c.url === "/api/artifacts:new")!.body).toEqual({ title: "Grocery splitter", brief: "Split a grocery receipt", build: "claxbot" });
    expect(assign).toHaveBeenCalledWith("/a/abc#run=r1");
    // Building with ClaxBot does not choose it for later Sends: that is asked.
    expect(localStorage.getItem("clax.sendTo.abc")).toBeNull();
  });

  it("switched off, makes the artifact as before and asks who builds it", async () => {
    const calls = stubFetch({ "GET /api/me/bot": meBot(), "POST /api/artifacts:new": [201, started], "GET /api/agents?artifact=abc": { agents: [{ handle: "claxbot", name: "ClaxBot", person: null, live: true, builtin: true }, { handle: "a_mine", name: "clax", person: null, live: true }] } });
    const m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    await settle();
    click(m.root.querySelector('[role="switch"]')!);
    expect(m.root.querySelector('button[type="submit"]')!.textContent).toBe("Create");
    fill(m);
    submit(m.root);
    await settle();
    expect(calls.find(c => c.url === "/api/artifacts:new")!.body).toEqual({ title: "Grocery splitter", brief: "Split a grocery receipt" });
    expect(m.root.querySelector("h2")!.textContent).toBe("Who builds it?");
    // ClaxBot is the switch's, not one of the agents picked here.
    expect([...m.root.querySelectorAll("button.agent .nm")].map(n => n.textContent)).toEqual(["clax"]);
  });

  it("unavailable, shows the switch off and disabled with why; without ClaxBot, no switch at all", async () => {
    stubFetch({ "GET /api/me/bot": meBot({ available: false, unavailable_reason: "no_credits", week: week({ spent_milli: 2_000_000, exhausted: true }) }) });
    let m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    await settle();
    const sw = m.root.querySelector<HTMLElement>('[role="switch"]')!;
    expect([sw.getAttribute("aria-checked"), sw.getAttribute("aria-disabled")]).toEqual(["false", "true"]);
    expect(m.root.querySelector("#na-bot-l")!.textContent).toContain("Weekly credits used · resets Mon Oct 26.");
    m.unmount();
    forget();
    stubFetch({});
    m = mount(NewArtifactDialog, { me: { owner: true }, onClose: vi.fn() });
    await settle();
    expect(m.root.querySelector('[role="switch"]')).toBeNull();
  });

  it("unavailable, links to the page that fixes it for this person's role", async () => {
    const fixOf = async (reason: "not_configured" | "model_not_priced", role: "owner" | "member") => {
      forget();
      stubFetch({ "GET /api/me/bot": meBot({ available: false, unavailable_reason: reason, role }) });
      const m = mount(NewArtifactDialog, { me: { owner: role === "owner" }, onClose: vi.fn() });
      await settle();
      const a = m.root.querySelector<HTMLAnchorElement>(".bot-fix");
      const got = a && [a.textContent, a.getAttribute("href")];
      m.unmount();
      return got;
    };
    expect(await fixOf("not_configured", "owner")).toEqual(["Set a model key under Admin → Provider →", "/admin?tab=provider"]);
    expect(await fixOf("not_configured", "member")).toEqual(["Use your own Anthropic key under You →", "/me"]);
    expect(await fixOf("model_not_priced", "owner")).toEqual(["Price it under Admin → Pricing →", "/admin?tab=pricing"]);
    expect(await fixOf("model_not_priced", "member")).toBeNull();
  });
});

describe("the New artifact tile", () => {
  beforeEach(() => { forget(); forgetViewer(); });
  it("shows for a browser that may create, says ClaxBot can build it, and opens the dialog", async () => {
    stubFetch({ "GET /api/viewers/me": { viewer: { public_id: "p_owner", owner: true }, may_create: true }, "GET /api/me/bot": meBot() });
    const m = mount(NewTile, {});
    await settle();
    const tile = m.root.querySelector<HTMLButtonElement>('button[aria-label="New artifact"]')!;
    expect(tile.textContent).toContain("Name it; ClaxBot can build it");
    click(tile);
    await vi.waitFor(() => expect(document.querySelector('[role="dialog"]')).not.toBeNull());
  });
  it("is not offered to a browser that may not create", async () => {
    stubFetch({ "GET /api/viewers/me": { viewer: null, may_create: false } });
    const m = mount(NewTile, {});
    await settle();
    expect(m.root.querySelector("button")).toBeNull();
  });
});

describe("titles", () => {
  it("take the zero-width non-joiner and joiner", () => {
    expect(formProblem("می‌خواهم", "x")).toBeNull();
    expect(formProblem("Team \u{1f468}‍\u{1f469}", "x")).toBeNull();
    expect(formProblem("a​b", "x")).toBe("A title is one line of plain text.");
  });
});
