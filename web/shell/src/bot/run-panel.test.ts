import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import { meBot, run } from "./fixtures";
import { forget } from "./me";
import { RunFeed } from "./run-feed.svelte";
import RunPanel from "./RunPanel.svelte";
import { BOT, botView, sendToBot, withBot } from "./send.svelte";
import { writable } from "svelte/store";
import type { ArtifactController } from "../view/artifact-controller";
import ArtifactBot from "./ArtifactBot.svelte";
import { nav } from "../nav";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const button = (root: Element, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === text)!;

/** A run's event stream the test writes to, one SSE event at a time. */
function sse() {
  let ctl!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({ start(c) { ctl = c; } });
  const enc = new TextEncoder();
  return { body, send: (event: string, data: unknown) => ctl.enqueue(enc.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)), end: () => ctl.close() };
}

type Call = { url: string; method: string; body?: unknown };
function daemon(routes: Record<string, (body: unknown) => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    calls.push({ url, method, body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const r = routes[`${method} ${url}`];
    return r ? r(init?.body) : new Response(JSON.stringify({ error: { code: "not_found", message: "not found" } }), { status: 404 });
  }));
  return calls;
}
const json = (v: unknown, status = 200) => () => new Response(JSON.stringify(v), { status });

beforeEach(() => { forget(); botView.run = null; botView.started = []; botView.agent = null; botView.me = null; botView.chosen = false; });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("the run panel", () => {
  it("follows a run live: narration, the live script open with its calls, the credits, and Stop", async () => {
    const stream = sse();
    const calls = daemon({
      "GET /api/bot/runs/r1": json(run()),
      "GET /api/bot/runs/r1/events": () => new Response(stream.body, { headers: { "content-type": "text/event-stream" } }),
      "POST /api/bot/runs/r1/cancel": json({ run: { ...run(), status: "stopping" } }),
    });
    botView.me = meBot();
    const m = mount(RunPanel, { aid: "7q3k9mzx2b4t", runs: [] });
    botView.open("r1");
    flush();
    await settle();
    const panel = document.querySelector<HTMLElement>("#bot-run-panel")!;
    expect(document.activeElement).toBe(panel.querySelector("h2"));
    expect(panel.querySelector(".st")!.textContent).toBe("Working");
    stream.send("step_started", { step: { n: 1, kind: "turn", status: "running", started_at: "x", credits_milli: 0 } });
    stream.send("turn_delta", { step: 1, text: "Reading the page and the totals code first." });
    stream.send("step_done", { step: { n: 1, kind: "turn", status: "done", started_at: "x", credits_milli: 0, text: "Reading the page and the totals code first." } });
    stream.send("step_started", { step: { n: 2, kind: "script", status: "running", started_at: "x", credits_milli: 0 } });
    stream.send("script", { step: 2, title: "Read the page and its data", code: "const html = await clax.files.read(\"index.html\");" });
    stream.send("call", { step: 2, call: { op: "files.read", target: "index.html", ok: true, ms: 12 } });
    await settle();
    expect(panel.querySelector(".say")!.textContent).toBe("Reading the page and the totals code first.");
    const card = panel.querySelector<HTMLDetailsElement>("li.script details")!;
    expect(card.open).toBe(true);
    expect(card.querySelector("summary")!.textContent).toContain("running · 1 call");
    expect(card.querySelector(".calls")!.textContent).toContain("files.read index.html12 ms");
    stream.send("step_done", { step: { n: 2, kind: "script", status: "done", started_at: "x", credits_milli: 1000, title: "Read the page and its data", calls: [{ op: "files.read", target: "index.html", ok: true, ms: 12 }], value: { size: 8312 } }, run: { credits_milli: 3000 } });
    await settle();
    expect(panel.querySelector("li.script summary")!.textContent).toContain("1 call · 12 ms");
    expect(panel.querySelector('pre[aria-label="Output"]')!.textContent).toContain("8312");
    expect(panel.querySelector(".cost")!.textContent).toBe("This run: 3 credits · 38k tokens on your key");
    expect(panel.querySelector(".wk")!.textContent).toContain("310 / 2,000");
    expect(panel.querySelector<HTMLAnchorElement>(".wk a")!.getAttribute("href")).toBe("/audit?artifact=7q3k9mzx2b4t&q=r1");
    click(button(panel, "Stop"));
    await settle();
    expect(calls.find(c => c.method === "POST")!.url).toBe("/api/bot/runs/r1/cancel");
    stream.send("run_end", { run: { status: "stopped", reason: "cancelled", draft: { url: "/a/7q3k9mzx2b4t?draft=claxbot" } } });
    await settle();
    expect(panel.querySelector(".st")!.textContent).toBe("Stopped: it was stopped");
    expect(panel.querySelector(".stop")).toBeNull();
    expect(panel.querySelector<HTMLAnchorElement>(".draft a")!.textContent).toBe("Open the draft");
    m.unmount();
  });

  it("shows a running turn's reasoning and what it is writing, with a clock that ticks", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"], now: Date.parse("2026-10-21T14:02:00Z") });
    try {
      const stream = sse();
      daemon({
        "GET /api/bot/runs/r1": json(run()),
        "GET /api/bot/runs/r1/events": () => new Response(stream.body, { headers: { "content-type": "text/event-stream" } }),
      });
      const m = mount(RunPanel, { aid: "7q3k9mzx2b4t", runs: [] });
      botView.open("r1");
      flush();
      await settle();
      const panel = document.querySelector<HTMLElement>("#bot-run-panel")!;
      stream.send("step_started", { step: { n: 1, kind: "turn", status: "running", started_at: "2026-10-21T14:02:00Z", credits_milli: 0 } });
      stream.send("turn_delta", { step: 1, type: "thinking", text: "The map needs the lines first." });
      await settle();
      expect(panel.querySelector(".think")!.textContent).toBe("The map needs the lines first.");
      expect(panel.querySelector(".activity")!.textContent).toBe("Thinking · 0:00");
      vi.advanceTimersByTime(3000);
      flush();
      expect(panel.querySelector(".activity")!.textContent).toBe("Thinking · 0:03");
      stream.send("turn_delta", { step: 1, type: "tool_input", text: '{"title":"Draw the lines","code":"a;\\nb;' });
      await settle();
      expect(panel.querySelector(".activity")!.textContent).toBe("Writing “Draw the lines” · 2 lines · 0:03");
      // The tool input never shows as words.
      expect(panel.querySelector(".say:not(.activity)")).toBeNull();
      stream.send("step_done", { step: { n: 1, kind: "turn", status: "done", started_at: "2026-10-21T14:02:00Z", credits_milli: 0 } });
      await settle();
      expect([panel.querySelector(".think"), panel.querySelector(".activity")]).toEqual([null, null]);
      m.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it("collapses a finished script, opens one that failed, and closes on Escape", async () => {
    daemon({ "GET /api/bot/runs/r2": json(run({ id: "r2", status: "finished", steps: [
      { n: 1, kind: "script", status: "done", started_at: "x", credits_milli: 0, title: "Look", calls: [], code: "return 1" },
      { n: 2, kind: "script", status: "done", started_at: "x", credits_milli: 0, title: "Write", calls: [{ op: "files.write", target: "index.html", ok: false, ms: 3, error: "conflict" }], error: "the version changed" },
      { n: 3, kind: "finish", status: "done", started_at: "x", credits_milli: 0, summary: "Fixed the totals." },
    ] })) });
    const m = mount(RunPanel, { aid: "7q3k9mzx2b4t", runs: [] });
    botView.open("r2");
    flush();
    await settle();
    const cards = [...document.querySelectorAll<HTMLDetailsElement>("#bot-run-panel details")];
    expect(cards.map(c => c.open)).toEqual([false, true]);
    expect(document.querySelector("#bot-run-panel .finish")!.textContent).toContain("Fixed the totals.");
    expect(document.querySelector("#bot-run-panel .stop")).toBeNull();
    flush(() => document.querySelector("#bot-run-panel")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(document.querySelector("#bot-run-panel")).toBeNull();
    expect(botView.run).toBeNull();
    m.unmount();
  });

  it("resumes after the last event it applied, without a gap, and drops a replay it has", async () => {
    const enc = new TextEncoder();
    const body = (events: [string, string, unknown][]) => new ReadableStream<Uint8Array>({ start(c) { for (const [id, e, d] of events) c.enqueue(enc.encode(`id: ${id}\nevent: ${e}\ndata: ${JSON.stringify(d)}\n\n`)); c.close(); } });
    const asked: (string | null)[] = [];
    let reads = 0;
    const streams = [
      // From the run's last_event_id (1.1:0): a replay of 1.1:0, then 1.1:1.
      body([["1.1:0", "turn_delta", { step: 1, text: "Reading" }], ["1.1:1", "turn_delta", { step: 1, text: " the page." }]]),
      // After 1.1:1, the server replays from it: 1.1:1 again, then 2.1:0.
      body([["1.1:1", "turn_delta", { step: 1, text: " the page." }], ["2.1:0", "step_started", { step: { n: 2, kind: "script", status: "running", started_at: "x", credits_milli: 0 } }]]),
    ];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/bot/runs/r3") { reads++; return new Response(JSON.stringify(run({ id: "r3", last_event_id: "1.1:0", steps: [{ n: 1, kind: "turn", status: "running", started_at: "x", credits_milli: 0, text: "Reading" }] }))); }
      if (url === "/api/bot/runs/r3/events") { asked.push(new Headers(init?.headers).get("last-event-id")); return new Response(streams.shift()!); }
      return new Response("{}", { status: 404 });
    }));
    const later: (() => void)[] = [];
    const f = new RunFeed("r3", fn => { later.push(fn); });
    f.start();
    await settle();
    expect(f.run!.steps[0].text).toBe("Reading the page.");
    expect(later).toHaveLength(1);
    later.shift()!();
    await settle();
    expect(asked).toEqual(["1.1:0", "1.1:1"]);
    // The run is read once: a reconnect resumes from the last event.
    expect(reads).toBe(1);
    expect(f.run!.steps.map(s => [s.n, s.text ?? null])).toEqual([[1, "Reading the page."], [2, null]]);
    expect(f.last).toBe("2.1:0");
    f.stop();
  });
});

describe("a step begun again after a restart", () => {
  it("replaces the old attempt's partial state with the new attempt's", async () => {
    const enc = new TextEncoder();
    const body = (events: [string, string, unknown][]) => new ReadableStream<Uint8Array>({ start(c) { for (const [id, e, d] of events) c.enqueue(enc.encode(`id: ${id}\nevent: ${e}\ndata: ${JSON.stringify(d)}\n\n`)); c.close(); } });
    const started = (at: string) => ({ step: { n: 2, kind: "turn", status: "running", started_at: at, credits_milli: 0 } });
    const streams = [
      body([["2.1:0", "step_started", started("a1")], ["2.1:1", "turn_delta", { step: 2, text: "Half a thou" }]]),
      // The daemon restarted: step 2 is begun again, attempt 2, from scratch.
      body([["2.2:0", "step_started", started("a2")], ["2.2:1", "turn_delta", { step: 2, text: "A whole thought." }]]),
    ];
    const asked: (string | null)[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/bot/runs/r4") return new Response(JSON.stringify(run({ id: "r4", last_event_id: "1.1:3", steps: [{ n: 1, kind: "turn", status: "done", started_at: "x", credits_milli: 0, text: "Looked." }] })));
      if (url === "/api/bot/runs/r4/events") { asked.push(new Headers(init?.headers).get("last-event-id")); return new Response(streams.shift()!); }
      return new Response("{}", { status: 404 });
    }));
    const later: (() => void)[] = [];
    const f = new RunFeed("r4", fn => { later.push(fn); });
    f.start();
    await settle();
    expect(f.run!.steps[1].text).toBe("Half a thou");
    later.shift()!();
    await settle();
    expect(asked).toEqual(["1.1:3", "2.1:1"]);
    expect(f.run!.steps.map(s => [s.n, s.started_at, s.text])).toEqual([[1, "x", "Looked."], [2, "a2", "A whole thought."]]);
    f.stop();
  });
});

describe("ClaxBot in the artifact view", () => {
  const view = (threads: unknown[] = [], agents: unknown[] = []) => ({ id: "7q3k9mzx2b4t", state: writable({ threads, agents }), selectThread: vi.fn() }) as unknown as ArtifactController;

  it("reads a run the artifact's list does not hold once, on its own, and never loops", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t#run=gone");
    const calls = daemon({ "GET /api/me/bot": json(meBot()), "GET /api/bot/runs?artifact=7q3k9mzx2b4t": json({ runs: [] }) });
    const m = mount(ArtifactBot, { ctl: view() });
    await settle();
    await settle();
    expect(botView.run).toBe("gone");
    expect(calls.filter(c => c.url.startsWith("/api/bot/runs?")).length).toBe(1);
    // The run alone, once by the list's reader and once by the panel's feed.
    expect(calls.filter(c => c.url === "/api/bot/runs/gone").length).toBeLessThanOrEqual(2);
    m.unmount();
    history.replaceState(null, "", "/");
  });

  it("claims its runs' threads, and loads the version a run this viewer follows published once it finishes", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t");
    const stream = sse();
    daemon({
      "GET /api/me/bot": json(meBot()),
      "GET /api/bot/runs?artifact=7q3k9mzx2b4t": json({ runs: [run({ thread_ids: ["t_brief"] })] }),
      "GET /api/bot/runs/r1": json(run({ thread_ids: ["t_brief"] })),
      "GET /api/bot/runs/r1/events": () => new Response(stream.body, { headers: { "content-type": "text/event-stream" } }),
    });
    const reload = vi.spyOn(nav, "reload").mockImplementation(() => {});
    const state = writable<Record<string, unknown>>({ threads: [], agents: [], me: { public_id: "p_sam", owner: false }, newer: null, deleted: false });
    const ctl = { id: "7q3k9mzx2b4t", state, selectThread: vi.fn(), here: () => "/a/7q3k9mzx2b4t" } as unknown as ArtifactController;
    const m = mount(ArtifactBot, { ctl });
    await settle();
    expect(botView.claimed).toEqual(["t_brief"]);
    botView.open("r1");
    flush();
    await settle();
    stream.send("run_end", { run: { status: "finished" } });
    await settle();
    // Finished, but its version is not out yet: nothing loads until it is.
    expect(reload).not.toHaveBeenCalled();
    state.update(s => ({ ...s, newer: 2 }));
    flush();
    expect(reload).toHaveBeenCalledWith("/a/7q3k9mzx2b4t#run=r1");
    m.unmount();
    reload.mockRestore();
    history.replaceState(null, "", "/");
  });

  it("shows a teammate's run read-only to someone ClaxBot is not on for, listed disabled with why", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t");
    const calls = daemon({ "GET /api/me/bot": json(meBot({ enabled: false, available: false, unavailable_reason: "no_flag" })), "GET /api/bot/runs?artifact=7q3k9mzx2b4t": json({ runs: [run()] }), "GET /api/bot/runs/r1": json(run()) });
    const ctl = { id: "7q3k9mzx2b4t", state: writable({ threads: [], agents: [{ handle: "claxbot", harness: "claxbot", name: "ClaxBot", live: false, builtin: true, unavailable: "no_flag" }], me: { public_id: "p_mia", owner: false } }), selectThread: vi.fn() } as unknown as ArtifactController;
    const m = mount(ArtifactBot, { ctl });
    await settle();
    expect(botView.agent).toMatchObject({ live: false, line: "Not turned on for you: an admin can turn it on" });
    // The artifact's runs are listed: Sam's run opens read-only, with no Stop.
    expect(calls.some(c => c.url === "/api/bot/runs?artifact=7q3k9mzx2b4t")).toBe(true);
    click(m.root.querySelector(".botbtn")!);
    await settle();
    const panel = document.querySelector("#bot-run-panel")!;
    expect(panel.querySelector(".stop")).toBeNull();
    expect(panel.textContent).toContain("Sam's run: you can follow it, and only they can stop it.");
    m.unmount();
  });

  it("names the threads a run answers, and shows one when chosen", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t#run=r1");
    const thread = { id: "t1", comments: [{ body: "The totals row should round to cents.\nAnd the arrows." }] };
    daemon({ "GET /api/me/bot": json(meBot()), "GET /api/bot/runs?artifact=7q3k9mzx2b4t": json({ runs: [run()] }), "GET /api/bot/runs/r1": json(run({ status: "finished" })) });
    const ctl = view([thread], [{ handle: "claxbot", harness: "claxbot", name: "ClaxBot", live: true, builtin: true }]);
    const m = mount(ArtifactBot, { ctl });
    await settle();
    await settle();
    const answers = document.querySelector('#bot-run-panel [aria-label="Threads it answers"]')!;
    expect(answers.textContent).toBe("“The totals row should round to cents.”");
    click(answers.querySelector("button")!);
    expect(ctl.selectThread).toHaveBeenCalledWith(thread);
    // ClaxBot is listed to the Send menu as the artifact's agents list it.
    expect(botView.agent).toMatchObject({ handle: BOT, live: true, builtin: true });
    m.unmount();
    history.replaceState(null, "", "/");
  });
});

describe("loading ClaxBot's part with the roster", () => {
  const state = (agents: unknown[]) => ({ data: { artifact: { participants: { people: [], agents }, live: null } }, agents, working: [], me: null, presence: [], viewing: 0, menu: null, attention: null, decided: null, deleted: false, newer: null, rally: false, narrow: false, threads: [] });
  const ctlOf = (s: ReturnType<typeof state>) => ({ id: "7q3k9mzx2b4t", state: writable(s), openMenu: vi.fn(), closeMenu: vi.fn(), selectThread: vi.fn() }) as unknown as ArtifactController;

  it("loads it where the artifact's agents list ClaxBot, though this tab heard it is off, and remembers the listing", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t");
    sessionStorage.setItem("clax.bot", "0");
    daemon({ "GET /api/me/bot": json(meBot({ enabled: true })), "GET /api/bot/runs?artifact=7q3k9mzx2b4t": json({ runs: [] }) });
    const s = state([{ handle: "claxbot", harness: "claxbot", name: "ClaxBot", live: true, builtin: true }]);
    const { default: Who } = await import("../ui/Who.svelte");
    const m = mount(Who, { ctl: ctlOf(s), s, open: 0 } as never);
    await vi.waitFor(() => expect(botView.agent).toMatchObject({ handle: BOT, live: true }));
    expect(sessionStorage.getItem("clax.bot")).toBe("1");
    m.unmount();
  });

  it("asks nothing more where this tab heard ClaxBot is off and the agents do not list it", async () => {
    history.replaceState(null, "", "/a/7q3k9mzx2b4t");
    sessionStorage.setItem("clax.bot", "0");
    const calls = daemon({});
    const s = state([]);
    const { default: Who } = await import("../ui/Who.svelte");
    const m = mount(Who, { ctl: ctlOf(s), s, open: 0 } as never);
    await settle();
    expect(calls.filter(c => c.url.startsWith("/api/me/bot") || c.url.startsWith("/api/bot/"))).toEqual([]);
    expect(botView.agent).toBeNull();
    m.unmount();
  });
});

describe("a Send to ClaxBot", () => {
  it("starts a run of the threads sent and opens its panel; a refusal shows in the notice", async () => {
    const calls = daemon({ "POST /api/bot/runs": json({ run: run({ id: "r9" }) }) });
    const host = { id: "7q3k9mzx2b4t", setNotice: vi.fn() };
    expect(await sendToBot(host, ["t1", "t2"])).toBe(true);
    expect(calls.at(-1)).toEqual({ url: "/api/bot/runs", method: "POST", body: { thread_ids: ["t1", "t2"], artifact_id: "7q3k9mzx2b4t" } });
    expect(botView.run).toBe("r9");
    daemon({ "POST /api/bot/runs": json({ error: { code: "credits_exhausted", message: "this week's credits are used up" } }, 403) });
    expect(await sendToBot(host, ["t1"])).toBe(false);
    expect(host.setNotice).toHaveBeenLastCalledWith("ClaxBot did not start: This week's credits are used up.");
  });

  it("opens the run already working on a thread sent again, starting none", async () => {
    const calls = daemon({ "POST /api/bot/runs": json({ error: { code: "run_active", message: "a ClaxBot run is already working on this thread", run: { id: "r7" } } }, 409) });
    const host = { id: "7q3k9mzx2b4t", setNotice: vi.fn() };
    expect(await sendToBot(host, ["t1"])).toBe(true);
    expect(calls.filter(c => c.method === "POST")).toHaveLength(1);
    expect(botView.run).toBe("r7");
    expect(host.setNotice).toHaveBeenLastCalledWith("ClaxBot is already working on this thread: its run is open.");
  });

  it("lists ClaxBot first among the artifact's agents, and remembers it as the choice", () => {
    botView.list("7q3k9mzx2b4t", true, null, meBot());
    const agents = withBot([{ handle: "a_cl", harness: "claude", live: true, name: "clax-claude" }]);
    expect(agents.map(a => a.handle)).toEqual([BOT, "a_cl"]);
    expect(agents[0]).toMatchObject({ name: "ClaxBot", builtin: true, line: "Runs in Clax · instance key · 1,690 credits left" });
    botView.choose("7q3k9mzx2b4t", true);
    botView.list("7q3k9mzx2b4t", true, null, meBot());
    expect(botView.chosen).toBe(true);
    // Unavailable, it is listed but never the choice.
    botView.list("7q3k9mzx2b4t", false, "no_flag", meBot());
    expect(botView.chosen).toBe(false);
  });
});
