import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { relativeTime } from "./format";
import { FakeWorker, workerWith } from "./test/fake-worker";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./test/timeouts";

describe("relativeTime", () => {
  const now = new Date("2026-09-28T12:00:00Z");
  it("buckets", () => {
    expect(relativeTime("2026-09-28T11:59:40Z", now)).toBe("just now");
    expect(relativeTime("2026-09-28T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-09-28T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("2026-09-26T12:00:00Z", now)).toBe("2 d ago");
    expect(relativeTime("2026-01-01T00:00:00Z", now)).toBe("2026-01-01");
  });
});

/** Polls until `check` returns a truthy value; fails after `WAIT_MS`. */
async function waitFor<T>(check: () => T | null | undefined | false, what = "condition"): Promise<T> {
  const deadline = Date.now() + WAIT_MS;
  for (;;) {
    const v = check();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise(r => setTimeout(r, 10));
  }
}
const ARTIFACTS = [
  { id: "7q3k9mzx2b4t", title: "Pinned one", description: "d", icon: "chart", pinned: true, current_version: 3, updated_at: "2026-09-28T11:00:00Z" },
  { id: "aaaaaaaaaaaa", title: "Other", description: "sales report", icon: null, pinned: false, current_version: 1, updated_at: "2026-09-27T11:00:00Z" },
];

type Call = { url: string; method: string; body?: string };
/** Fields a single-artifact list answer changes, by artifact ID. */
let edits: Record<string, Record<string, unknown>> = {};
const ATTENTION = { artifacts: { aaaaaaaaaaaa: { addressed: [], addressed_v: null, new_replies: ["t"], open_in: ["t"], seen: 1 } } };
/** The inbox's owner routes: the summary and open questions, or 403 (not the owner). */
type Inbox = { summary: Record<string, unknown>; questions: unknown[] } | 403;
function stubApi(tokenStatus: number = 200, attention: "ok" | "fail" | "none" = "none", inbox?: Inbox) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, method: init?.method ?? "GET", body: init?.body as string | undefined });
    if (inbox && /^\/api\/(inbox|questions)/.test(url)) {
      if (inbox === 403) return new Response(JSON.stringify({ error: { code: "forbidden" } }), { status: 403 });
      return new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? inbox.summary : { questions: inbox.questions, open: inbox.questions.length }));
    }
    if (url.endsWith("/api/viewers/me/attention")) {
      if (attention === "fail") throw new Error("network");
      if (attention === "ok") return new Response(JSON.stringify(ATTENTION));
    }
    if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: ARTIFACTS }));
    const one = url.match(/^\/api\/artifacts\?artifact=(\w+)$/);
    if (one) return new Response(JSON.stringify({ artifacts: ARTIFACTS.filter(a => a.id === one[1]).map(a => ({ ...a, ...edits[a.id] })) }));
    if (url.startsWith("/api/viewers/me/attention?artifact=")) return new Response(JSON.stringify({ artifacts: {} }));
    if (url.includes("haiku")) return new Response(JSON.stringify(["one\ntwo\nthree"]));
    if (url.endsWith("/api/viewers/me")) return new Response(JSON.stringify({ viewer: { public_id: "v1", display_name: "Ada", created_at: "2026-09-01T00:00:00Z" } }));
    if (url.endsWith("/api/token")) return tokenStatus === 200 ? new Response(JSON.stringify({ token: "t" })) : new Response("{}", { status: tokenStatus });
    if (init?.method === "PATCH") return new Response(JSON.stringify({ artifact: ARTIFACTS[0] }));
    if (init?.method === "DELETE") return new Response(null, { status: 204 });
    return new Response("{}", { status: 404 });
  }));
  return calls;
}

// The mounted gallery's teardown: its footer haiku resolves a lazy import, so
// each test unmounts its gallery before the next resets the module registry.
let unmountGallery: (() => void) | null = null;

async function mountGallery() {
  const { default: Gallery } = await import("./ui/Gallery.svelte");
  // From the same module registry as the gallery (each test resets it), so
  // both use one Svelte runtime.
  const { mount } = await import("./test/svelte");
  const { root, unmount } = mount(Gallery, {});
  unmountGallery = unmount;
  await waitFor(() => root.querySelector("a.card, .empty, .empty-gallery"), "gallery to render");
  return root;
}

describe("Gallery", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { unmountGallery?.(); unmountGallery = null; vi.unstubAllGlobals(); document.body.replaceChildren(); edits = {}; });

  it("renders cards led by the version numeral, with title and link, and no description", async () => {
    stubApi();
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards.length).toBe(2);
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[0].textContent).toContain("Pinned one");
    expect(cards[0].querySelector(".v")?.textContent).toBe("v3");
    expect(cards[0].querySelector("p")).toBeNull();
    // One star: the Pin button's once the token is known, not the title's too.
    await waitFor(() => root.querySelector('.card-tools button[title="Unpin"]'), "the Unpin button");
    expect(root.querySelectorAll(".card-wrap")[0].textContent!.match(/★/g)).toHaveLength(1);
    expect(cards[0].querySelector(".pin")).toBeNull();
    expect(cards[1].querySelector(".by")?.textContent).toContain("command line");
    await waitFor(() => root.querySelector(".gbar .sub")?.textContent?.includes("seen as Ada"), "the viewer's name in the bar");
    expect(root.querySelector(".gbar .sub")?.textContent).toBe("local artifacts · seen as Ada");
  });

  it("puts the pinned artifact first even when the API lists it second", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [ARTIFACTS[1], ARTIFACTS[0]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].getAttribute("href")).toBe("/a/7q3k9mzx2b4t");
    expect(cards[1].getAttribute("href")).toBe("/a/aaaaaaaaaaaa");  });

  it("shows one three-line haiku in the footer once the list loads", async () => {
    stubApi();
    const root = await mountGallery();
    const pre = await waitFor(() => root.querySelector(".gfoot pre"), "the footer haiku");
    expect(pre.textContent!.split("\n")).toHaveLength(3);
  });

  it("labels agent-published cards with the harness and a green dot only while the session is live", async () => {
    const owned = (id: string, live: boolean) => ({ ...ARTIFACTS[1], id, owner_session_id: "s" + id, owner_live: live, owner_harness: "claude-code" });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [owned("live00000000", true), owned("gone00000000", false), ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].querySelector(".by")?.textContent).toContain("claude-code");
    expect(cards[0].querySelector(".live-dot")).not.toBeNull();
    expect(cards[1].querySelector(".by")?.textContent).toContain("claude-code");
    expect(cards[1].querySelector(".live-dot")).toBeNull();
    expect(cards[2].querySelector(".by")?.textContent).not.toContain("claude-code");
    expect(cards[2].querySelector(".by")?.textContent).toContain("command line");
  });

  it("marks a live page's card Live and names its page by URL", async () => {
    const live = { ...ARTIFACTS[1], id: "live00000000", title: "Settings", kind: "live", live: { origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings" } };
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [live, ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    // The chip loads after the first paint, with the other markers.
    expect((await waitFor(() => cards[0].querySelector(".chip.live"), "the Live chip")).textContent).toBe("Live");
    expect(cards[0].querySelector(".by")?.textContent).toContain("localhost:5173/settings");
    expect(cards[0].querySelector(".by")?.textContent).not.toContain("command line");
    expect(cards[1].querySelector(".chip.live")).toBeNull();
  });

  it("shows a working chip on a card an agent works on, none on the others, and the roster in the footer", async () => {
    const working = [{ key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-28T11:00:00Z", last_heartbeat: "2026-09-28T11:00:00Z" }];
    const parts = { people: [{ public_id: "v1", display_name: "Ada", seen: null }], agents: [{ handle: "a_1111aaaa", harness: "claude", live: true }] };
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [{ ...ARTIFACTS[0], working, participants: parts }, ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    // The chips and rosters load after the first paint.
    await waitFor(() => cards[0].querySelector(".chip.ag"), "the working chip");
    expect(Array.from(cards[0].querySelectorAll(".chip.ag")).map(c => c.textContent)).toEqual(["claude working"]);
    expect(cards[1].querySelector(".chip.ag")).toBeNull();
    expect(cards[1].querySelector(".mks")).toBeNull();
    expect(cards[0].querySelector(".ft .agt .tok.work")?.textContent).toBe("cl");
  });

  it("floats what needs this viewer's eyes above everything else, with its markers and the version last seen", async () => {
    stubApi(200, "ok");
    const root = await mountGallery();
    const needs = await waitFor(() => root.querySelector(".grp.needs"), "the needs group");
    expect(needs.querySelector("h2")?.textContent).toContain("Needs your eyes");
    const other = needs.querySelector(".card-wrap")!;
    expect(other.textContent).toContain("Other");
    expect(other.querySelector(".chip.rep")?.textContent).toBe("1 new reply");
    expect(other.querySelector(".chip.oth")?.textContent).toBe("1 open");
    expect(other.querySelector(".ft .seen")?.textContent).toBe("seen v1");
    const rest = root.querySelector(".grp.rest")!;
    expect(rest.querySelector("h2")?.textContent).toContain("Everything else");
    expect(rest.textContent).toContain("Pinned one");
    expect(rest.textContent).not.toContain("Other");
  });

  it("leads with the inbox's unread summary for the owner: the two oldest questions as cards, then the newest rows, then how many more", async () => {
    const { item, view } = await import("./q/fixtures");
    const qs = [1, 2, 3].map(n => view({ id: `Q${n}`, created_at: `2026-10-07T09:0${n}:00.000Z` }));
    FakeWorker.all = [];
    vi.stubGlobal("SharedWorker", FakeWorker);
    stubApi(200, "ok", { summary: { unread: 5, questions: qs, latest: [item("reply")] }, questions: qs });
    const root = await mountGallery();
    (await workerWith("inbox", "questions")).live(["inbox", "questions"]);
    const sum = await waitFor(() => root.querySelector<HTMLElement>(".inbox-sum:not([hidden]) .qcard") && root.querySelector<HTMLElement>(".inbox-sum"), "the inbox summary");
    const needs = await waitFor(() => root.querySelector(".grp.needs"), "the needs group");
    expect(sum.compareDocumentPosition(needs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(sum.querySelector("h2")!.textContent).toBe("Inbox · 5 unread");
    expect([...sum.querySelectorAll<HTMLElement>(".qcard")].map(c => c.dataset.question)).toEqual(["Q1", "Q2"]);
    const more = [...sum.querySelectorAll<HTMLAnchorElement>("a.more")];
    expect(more.map(a => [a.textContent, a.getAttribute("href")])).toEqual([["1 more question in the inbox", "/inbox?kind=question"], ["1 more in the inbox", "/inbox"]]);
    expect([...sum.querySelectorAll(".irow")].map(r => r.getAttribute("data-item"))).toEqual([item("reply").id]);
    // And Inbox in the header, with the count.
    expect(root.querySelector(".gbar .inbox-link .count")!.textContent).toBe("5");
  });

  it("shows no summary when nothing is unread", async () => {
    FakeWorker.all = [];
    vi.stubGlobal("SharedWorker", FakeWorker);
    stubApi(200, "ok", { summary: { unread: 0, questions: [], latest: [] }, questions: [] });
    const root = await mountGallery();
    (await workerWith("inbox", "questions")).live(["inbox", "questions"]);
    await waitFor(() => root.querySelector(".gbar .inbox-link"), "the Inbox link");
    expect(root.querySelector<HTMLElement>(".inbox-sum")!.hidden).toBe(true);
    expect(root.querySelector(".gbar .inbox-link .count")).toBeNull();
  });

  it("loads nothing of the inbox for a browser that is not the owner's", async () => {
    FakeWorker.all = [];
    vi.stubGlobal("SharedWorker", FakeWorker);
    // No token: not the owner's browser (a LAN viewer).
    const calls = stubApi(403, "ok", 403);
    const root = await mountGallery();
    await workerWith("gallery");
    await waitFor(() => calls.some(c => c.url === "/api/token"), "the token request");
    await new Promise(r => setTimeout(r, 50));
    expect(root.querySelector(".inbox-sum, .inbox-link")).toBeNull();
    expect(FakeWorker.all.some(w => w.topics.includes("inbox"))).toBe(false);
    expect(calls.some(c => /^\/api\/(inbox|questions)/.test(c.url))).toBe(false);
  });

  it("shows no inbox at /inbox to a browser that is not the owner's, and fetches nothing of it", async () => {
    history.replaceState(null, "", "/inbox");
    try {
      const calls = stubApi(403, "ok", 403);
      const root = await mountGallery();
      expect(root.querySelector("main .empty")!.textContent).toContain("The inbox is its owner's");
      expect(root.querySelector(".inbox, .inbox-link")).toBeNull();
      expect(calls.some(c => /^\/api\/(inbox|questions|artifacts)/.test(c.url))).toBe(false);
    } finally {
      history.replaceState(null, "", "/");
    }
  });

  it("without attention shows every card in one group and no needs group", async () => {
    stubApi(200, "fail");
    const root = await mountGallery();
    // The grouping loads with the rosters, after the first paint.
    await waitFor(() => root.querySelector(".ft .ros"), "the grouping to load");
    expect(root.querySelector(".grp.needs")).toBeNull();
    expect(root.querySelectorAll(".card-wrap").length).toBe(2);
    expect(root.querySelector(".grp.rest h2")?.textContent).toContain("Artifacts");
  });

  it("shows the mark with its halves apart when the gallery is empty", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => url.endsWith("/api/artifacts") ? new Response(JSON.stringify({ artifacts: [] })) : new Response("{}", { status: 404 })));
    const root = await mountGallery();
    expect(root.querySelector(".empty-gallery .mark[data-apart]")).not.toBeNull();
    expect(root.textContent).toContain("When an agent publishes a page, it lands here.");
    expect(root.textContent).toContain("clax publish");
  });

  it("marks a tenth version with the rally chip", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url.endsWith("/api/artifacts")) return new Response(JSON.stringify({ artifacts: [{ ...ARTIFACTS[0], current_version: 10 }, ARTIFACTS[1]] }));
      return new Response("{}", { status: 404 });
    }));
    const root = await mountGallery();
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].querySelector(".chip.rally")?.textContent).toBe("rally of 10");
    expect(cards[1].querySelector(".chip.rally")).toBeNull();
  });

  it("search narrows cards by title and description", async () => {
    stubApi();
    const root = await mountGallery();
    const input = root.querySelector("input[type=search]") as HTMLInputElement;
    expect(input.placeholder).toBe("Search artifacts");
    expect(input.getAttribute("aria-label")).toBe("Search artifacts");
    input.value = "SALES";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => root.querySelectorAll("a.card").length === 1, "one card");
    const cards = root.querySelectorAll("a.card");
    expect(cards[0].textContent).toContain("Other");
    input.value = "pinned";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => root.querySelector("a.card")?.textContent?.includes("Pinned one"), "pinned card");
    expect(root.querySelectorAll("a.card").length).toBe(1);
  });

  it("without a token renders no buttons", async () => {
    const calls = stubApi(403);
    const root = await mountGallery();
    await waitFor(() => calls.some(c => c.url.endsWith("/api/token")), "token request");
    await new Promise(r => setTimeout(r, 0));
    expect(root.querySelectorAll(".card-wrap").length).toBe(2);
    expect(root.querySelectorAll(".card-tools button").length).toBe(0);
    // Without the Pin button, the title carries the pinned star.
    expect(root.querySelectorAll(".card-wrap")[0].querySelector(".pin")).not.toBeNull();
    expect(root.querySelectorAll(".card-wrap")[0].textContent!.match(/★/g)).toHaveLength(1);
  });

  it("pin button sends PATCH with pinned true and refetches that card", async () => {
    const calls = stubApi();
    const root = await mountGallery();
    // The live refreshes load with the rosters, after the first paint.
    await waitFor(() => root.querySelector(".ft .ros"), "the live module to load");
    const pin = await waitFor(() => root.querySelectorAll(".card-wrap")[1]?.querySelector('button[title="Pin"]') as HTMLButtonElement | null, "pin button");
    const wrap = root.querySelectorAll(".card-wrap")[1];
    expect(wrap.querySelector("a button")).toBeNull();
    expect(pin.getAttribute("aria-label")).toBe("Pin Other");
    edits = { aaaaaaaaaaaa: { pinned: true } };
    pin.click();
    await waitFor(() => root.querySelector('button[aria-label="Unpin Other"]'), "the card to show its pin");
    const patch = calls.find(c => c.method === "PATCH")!;
    expect(patch.url).toBe("/api/artifacts/aaaaaaaaaaaa");
    expect(JSON.parse(patch.body!)).toEqual({ pinned: true });
    expect(calls.filter(c => c.method === "GET" && c.url.startsWith("/api/artifacts")).map(c => c.url)).toEqual(["/api/artifacts", "/api/artifacts?artifact=aaaaaaaaaaaa"]);
  });

  it("a version on the gallery topic updates its card in place, fetching only this viewer's attention on it", async () => {
    FakeWorker.all = [];
    vi.stubGlobal("SharedWorker", FakeWorker);
    const calls = stubApi(200, "ok");
    const root = await mountGallery();
    // The question module holds its topics too, which have not gone live.
    const w = await workerWith("gallery", "inbox", "questions");
    const before = calls.length;
    w.emit("gallery", "version", { artifact_id: "aaaaaaaaaaaa", n: 2, title: "Other", at: "2026-09-28T11:30:00Z" });
    const card = () => Array.from(root.querySelectorAll("a.card")).find(c => c.textContent!.includes("Other"));
    await waitFor(() => card()?.querySelector(".v")?.textContent === "v2", "the card to show v2");
    await waitFor(() => calls.length > before, "the attention request");
    expect(calls.slice(before).map(c => c.url).sort()).toEqual(["/api/viewers/me/attention?artifact=aaaaaaaaaaaa"]);
    expect(Array.from(root.querySelectorAll("a.card")).find(c => c.textContent!.includes("Pinned one"))?.querySelector(".v")?.textContent).toBe("v3");
  });

  it("delete calls DELETE only after confirm", async () => {
    const calls = stubApi();
    const confirmSpy = vi.fn(() => false);
    vi.stubGlobal("confirm", confirmSpy);
    const root = await mountGallery();
    const del = await waitFor(() => root.querySelector('button[title="Delete"]') as HTMLButtonElement | null, "delete button");
    del.click();
    expect(confirmSpy).toHaveBeenCalledWith('Delete "Pinned one"? This removes every version.');
    expect(calls.some(c => c.method === "DELETE")).toBe(false);
    confirmSpy.mockReturnValue(true);
    del.click();
    const d = await waitFor(() => calls.find(c => c.method === "DELETE"), "DELETE request");
    expect(d.url).toBe("/api/artifacts/7q3k9mzx2b4t");
  });
});

describe("getToken", () => {
  beforeEach(() => { vi.resetModules(); });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("retries after a transient failure", async () => {
    let n = 0;
    vi.stubGlobal("fetch", vi.fn(async () => {
      if (n++ === 0) throw new Error("network");
      return new Response(JSON.stringify({ token: "tok" }));
    }));
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
    expect(await getToken()).toBe("tok");
  });

  it("caches a definitive 403", async () => {
    const f = vi.fn(async () => new Response("{}", { status: 403 }));
    vi.stubGlobal("fetch", f);
    const { getToken } = await import("./api");
    expect(await getToken()).toBeNull();
    expect(await getToken()).toBeNull();
    expect(f).toHaveBeenCalledTimes(1);
  });
});
