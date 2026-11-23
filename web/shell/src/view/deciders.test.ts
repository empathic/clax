import { afterEach, describe, expect, it, vi } from "vitest";

const seedBot = vi.fn();
const myBot = vi.fn(async () => null);
vi.mock("../bot/me", () => ({ seedBot, myBot }));
const inboxBrowser = vi.fn(async () => true);
vi.mock("../owner", () => ({ inboxBrowser }));
const inboxSummary = vi.fn(async () => ({ unread: 3, questions: [], latest: [] }));
const listQuestions = vi.fn(async () => ({ questions: [], open: 0 }));
vi.mock("../q/api", () => ({ inboxSummary, listQuestions }));

const ID = "7q3k9mzx2b4t";
const block = {
  status: { feedback: 200, bot: 200, hosted: 200, summary: 200, questions: 200, clone: 200, runs: 200 },
  feedback: { enabled: true, instance: "https://clax.example", queue: "q1", user: "u" },
  bot: { enabled: true },
  hosted: null,
  summary: { unread: 2, questions: [], latest: [] },
  questions: { questions: [], open: 0 },
  clone: { available: true, scopes: ["read"] },
  runs: { runs: [{ id: "r1" }] },
  inbox: true,
  owner: false,
};

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); seedBot.mockClear(); inboxBrowser.mockClear(); document.documentElement.removeAttribute("data-inbox"); });

/** A fetch that answers 401 to everything, and counts what it was asked. */
const refuse = () => {
  const f = vi.fn(async (_url: string) => new Response("{}", { status: 401 }));
  vi.stubGlobal("fetch", f);
  return f;
};

describe("what decides the artifact view's optional parts", () => {
  it("takes the block's answers at once: Inbox and its count, the owner's token, the feedback menu, Clone, ClaxBot and its runs; asking none of their routes", async () => {
    const f = refuse();
    const d = await import("./deciders.svelte");
    d.startDeciders(ID, block as never);
    expect([d.decided.inbox, d.decided.unread, d.decided.owner, d.decided.feedback]).toEqual([true, 2, false, true]);
    expect(document.documentElement.hasAttribute("data-inbox")).toBe(true);
    expect(d.cloneKnown()).toEqual(block.clone);
    expect(d.botKnown()).toEqual(block.bot);
    expect(d.runsKnown()).toEqual(block.runs.runs);
    expect(await d.firstInbox()).toEqual({ summary: block.summary, open: [] });
    await vi.waitFor(() => expect(seedBot).toHaveBeenCalledWith(block.bot));
    // The token alone is asked (for Publish, the local owner's), none of the deciders' routes.
    expect(f.mock.calls.map(c => String(c[0]))).toEqual(["/api/token"]);
    await d.decideInbox();
    expect(inboxBrowser).not.toHaveBeenCalled();
  });

  it("without the block, asks as the page starts, and decides Inbox beside the view's data", async () => {
    const f = refuse();
    const d = await import("./deciders.svelte");
    d.startDeciders(ID, null);
    expect([d.decided.inbox, d.decided.unread, d.decided.feedback]).toEqual([null, null, null]);
    expect(d.cloneKnown()).toBeUndefined();
    expect(d.botKnown()).toBeUndefined();
    expect(d.runsKnown()).toBeUndefined();
    await d.decideInbox(Promise.resolve(null));
    expect(d.decided.inbox).toBe(true);
    expect((await d.firstInbox())?.summary?.unread).toBe(3);
    expect(d.decided.unread).toBe(3);
    await vi.waitFor(() => expect(d.cloneKnown()).toBeNull());
    // What the view waits for beside its data (Publish's, ClaxBot's runs, the feedback menu's): refused, each decides its control absent.
    await d.barDecided();
    expect([d.runsKnown(), d.remotesKnown(), d.decided.feedback]).toEqual([[], null, false]);
    expect(f.mock.calls.map(c => String(c[0]))).toEqual(expect.arrayContaining(["/_app/feedback/config", `/api/artifacts/${ID}/git-token`, `/api/bot/runs?artifact=${ID}`]));
  });

  it("a browser without an inbox has its questions decided at once", async () => {
    refuse();
    const d = await import("./deciders.svelte");
    d.startDeciders(ID, { ...block, inbox: false } as never);
    expect([d.decided.inbox, d.decided.questions]).toEqual([false, true]);
    expect(document.documentElement.hasAttribute("data-inbox")).toBe(false);
  });
});
