import { describe, expect, it, vi } from "vitest";
import { ID, artifact, artifactViewHooks, mountView, stubMedia, viewer, waitFor } from "./test/artifact-view";
import GetInClax from "./ui/GetInClax.svelte";
import { flush, mount } from "./test/svelte";
import { MOUNT_TIMEOUT_MS } from "./test/timeouts";
import MoreMenu from "./ui/MoreMenu.svelte";
import PhoneTabs from "./ui/PhoneTabs.svelte";
import { InboxFeed } from "./q/feed.svelte";
import InboxLink from "./q/InboxLink.svelte";

describe("Echo top bar parts", () => {
  it("Inbox links to /inbox with the unread count, hidden at 0, and shows to the owner only", async () => {
    let summary: [number, unknown] = [200, { unread: 3, questions: [], latest: [] }];
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(summary[1]), { status: summary[0] })));
    const feed = new InboxFeed();
    await feed.summary();
    const m = mount(InboxLink, { feed });
    const a = m.root.querySelector<HTMLAnchorElement>("a.inbox-link")!;
    expect(a.getAttribute("href")).toBe("/inbox");
    expect(a.querySelector(".count")!.textContent).toBe("3");
    expect(a.getAttribute("aria-label")).toBe("Inbox, 3 unread");
    flush(() => { feed.unread = 0; });
    expect(a.querySelector(".count")).toBeNull();
    expect(a.getAttribute("aria-label")).toBe("Inbox");
    summary = [403, {}];
    await feed.summary();
    flush();
    expect(m.root.querySelector("a.inbox-link")).toBeNull();
    m.unmount();
    vi.unstubAllGlobals();
  });

  it("the more menu holds open raw and copy link, and closes on Escape with focus back", () => {
    const onCopy = vi.fn();
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy });
    const b = m.root.querySelector("button.icon") as HTMLButtonElement;
    expect(b.getAttribute("aria-label")).toBe("More");
    expect(b.getAttribute("aria-expanded")).toBe("false");
    flush(() => b.click());
    const menu = m.root.querySelector("[role=menu]")!;
    expect(menu.querySelector("a")!.getAttribute("href")).toBe("/c/x/v/1/");
    expect(menu.querySelector("a")!.getAttribute("target")).toBe("_blank");
    flush(() => (menu.querySelector("button") as HTMLButtonElement).click());
    expect(onCopy).toHaveBeenCalled();
    flush(() => b.click());
    flush(() => m.root.querySelector("[role=menu]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    m.unmount();
  });

  it("the more menu keeps a disabled, focusable open raw for a deleted artifact", async () => {
    const m = mount(MoreMenu, { rawHref: null, canCopy: false, onCopy: vi.fn() });
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    await new Promise(r => setTimeout(r));
    expect(m.root.querySelector("[role=menu] a")).toBeNull();
    const raw = m.root.querySelector<HTMLButtonElement>("[role=menuitem]")!;
    expect(raw.textContent).toBe("Open raw");
    expect(raw.getAttribute("aria-disabled")).toBe("true");
    expect(document.activeElement).toBe(raw);
    m.unmount();
  });

  it("the more menu's items are out of the Tab order; Open raw closes it with focus back, and Shift+Tab closes it on its button", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const b = m.root.querySelector<HTMLButtonElement>("button.icon")!;
    flush(() => b.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.getAttribute("tabindex"))).toEqual(["-1", "-1"]);
    // Open raw opens a new tab; the menu closes with focus on its button.
    const raw = items[0] as HTMLAnchorElement;
    raw.addEventListener("click", e => e.preventDefault());
    flush(() => raw.click());
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    flush(() => b.click());
    await new Promise(r => setTimeout(r));
    const tab = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    flush(() => document.activeElement!.dispatchEvent(tab));
    expect(tab.defaultPrevented).toBe(true);
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    m.unmount();
  });

  it("at phone width the more menu leads with Versions, which closes it and opens the version menu; wider, it has no Versions item", async () => {
    let phone = true;
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: phone && q === "(max-width: 700px)" }));
    const onVersions = vi.fn();
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn(), onVersions });
    const b = m.root.querySelector<HTMLButtonElement>("button.icon")!;
    flush(() => b.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.textContent)).toEqual(["Versions", "Open raw", "Copy link"]);
    expect(document.activeElement).toBe(items[0]);
    flush(() => items[0].click());
    expect(onVersions).toHaveBeenCalledTimes(1);
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    phone = false;
    flush(() => b.click());
    expect(Array.from(m.root.querySelectorAll("[role=menuitem]")).map(i => i.textContent)).toEqual(["Open raw", "Copy link"]);
    m.unmount();
    vi.unstubAllGlobals();
  });

  it("in the Clax app the more menu has no Open raw or Copy link, and shows at phone width only", async () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(max-width: 700px)" }));
    const items = async () => {
      const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn(), onVersions: vi.fn() });
      flush(() => m.root.querySelector<HTMLButtonElement>("button.icon")!.click());
      await new Promise(r => setTimeout(r));
      const out = { items: Array.from(m.root.querySelectorAll("[role=menuitem]")).map(i => i.textContent), phoneOnly: m.root.querySelector(".more")!.classList.contains("phone-only") };
      m.unmount();
      return out;
    };
    expect(await items()).toEqual({ items: ["Versions", "Open raw", "Copy link"], phoneOnly: false });
    // What `installTransport` leaves on a shell document in the app.
    Object.defineProperty(window, "claxTransport", { configurable: true, value: Object.freeze({ embed: Object.freeze({ transport: "host", shell: "clax://app", content: "clax://{id}.c" }) }) });
    try {
      expect(await items()).toEqual({ items: ["Versions"], phoneOnly: true });
    } finally {
      delete (window as { claxTransport?: unknown }).claxTransport;
      vi.unstubAllGlobals();
    }
  });

  it("at phone width on a live page the more menu reads Snapshots and lists Open page", async () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(max-width: 700px)" }));
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn(), onVersions: vi.fn(), pageHref: "http://localhost:5173/settings" });
    flush(() => m.root.querySelector<HTMLButtonElement>("button.icon")!.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.textContent)).toEqual(["Snapshots", "Open page", "Open raw", "Copy link"]);
    expect(items[1].getAttribute("href")).toBe("http://localhost:5173/settings");
    m.unmount();
    vi.unstubAllGlobals();
  });

  it("at phone width on an app's page the more menu lists Get in Clax first", async () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(max-width: 700px)" }));
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn(), onVersions: vi.fn(), getHref: "/a/x/get" });
    flush(() => m.root.querySelector<HTMLButtonElement>("button.icon")!.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.textContent)).toEqual(["Get in Clax", "Versions", "Open raw", "Copy link"]);
    expect(items[0].getAttribute("href")).toBe("/a/x/get");
    m.unmount();
    vi.unstubAllGlobals();
  });

  it("C and ? typed in the open more menu do not reach the shell's keys", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const outsideKeys = vi.fn();
    window.addEventListener("keydown", outsideKeys);
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    await new Promise(r => setTimeout(r));
    for (const key of ["c", "C", "?"]) flush(() => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
    expect(outsideKeys).not.toHaveBeenCalled();
    expect(m.root.querySelector("[role=menu]")).not.toBeNull();
    window.removeEventListener("keydown", outsideKeys);
    m.unmount();
  });

  it("the more menu moves between its items with the arrow keys, keeps them from the shell, and closes when focus leaves it", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const outsideKeys = vi.fn();
    window.addEventListener("keydown", outsideKeys);
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    // Opening focuses the first item once the menu is drawn.
    await new Promise(r => setTimeout(r));
    const [raw, copy] = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(document.activeElement).toBe(raw);
    const press = (key: string) => flush(() => (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
    press("ArrowDown");
    expect(document.activeElement).toBe(copy);
    press("ArrowDown");
    expect(document.activeElement).toBe(raw);
    press("End");
    expect(document.activeElement).toBe(copy);
    press("ArrowUp");
    expect(document.activeElement).toBe(raw);
    expect(outsideKeys).not.toHaveBeenCalled();
    const elsewhere = document.body.appendChild(document.createElement("button"));
    flush(() => elsewhere.focus());
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    window.removeEventListener("keydown", outsideKeys);
    elsewhere.remove();
    m.unmount();
  });

  it("phone tabs are a labelled group that switches between the page and the threads", () => {
    const onPage = vi.fn();
    const onThreads = vi.fn();
    const props = { panel: false, open: 3, onPage, onThreads };
    const m = mount(PhoneTabs, props);
    const group = m.root.querySelector("[role=group]")!;
    expect(group.getAttribute("aria-label")).toBe("Page or threads");
    expect(m.root.querySelector("nav")).toBeNull();
    const [page, threads] = Array.from(group.querySelectorAll("button"));
    expect(page.getAttribute("aria-pressed")).toBe("true");
    expect(threads.getAttribute("aria-pressed")).toBe("false");
    expect(threads.textContent).toContain("3");
    flush(() => threads.click());
    expect(onThreads).toHaveBeenCalledOnce();
    m.update({ ...props, panel: true });
    expect(page.getAttribute("aria-pressed")).toBe("false");
    expect(threads.getAttribute("aria-pressed")).toBe("true");
    flush(() => page.click());
    expect(onPage).toHaveBeenCalledOnce();
    m.unmount();
  });
});

describe("the top bar on a live page", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  const live = { origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings" };
  const liveArtifact = () => {
    const a = artifact(2);
    return { artifact: { ...a.artifact, kind: "live", live }, versions: [{ ...a.versions[0], n: 1, created_at: "2026-10-05T10:00:00Z", note: "snapshot" }, { ...a.versions[0], created_at: "2026-10-05T10:01:00Z", note: "snapshot" }] };
  };

  it("disables Comment with the extension hint, links the page, and C does not enter comment mode", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(liveArtifact())));
    const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.comment"), "the Comment button");
    expect(comment.disabled).toBe(true);
    expect(comment.title).toBe("Comment on the live page with the Clax extension");
    // The link comes with the roster, which the entry loads after the first paint (`artifact-main.ts`).
    (await import("./ui/more-menu.svelte")).loadMoreMenu();
    const open = await waitFor(() => Array.from(view.root.querySelectorAll<HTMLAnchorElement>("a")).find(a => a.textContent === "Open page"), "the Open page link");
    expect(open.getAttribute("href")).toBe(live.page_url);
    expect(open.getAttribute("target")).toBe("_blank");
    expect(open.getAttribute("rel")).toContain("noopener");
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true }));
    expect(comment.getAttribute("aria-pressed")).toBe("false");
    expect(view.root.querySelector(".topbar.commenting")).toBeNull();
  });

  it("on a page a join merged away, links the page it was merged into instead of the page, and comments nowhere", async () => {
    const merged = () => { const a = liveArtifact(); return { ...a, artifact: { ...a.artifact, live: { ...live, merged_into: "8r4m0nzy3c5v", merged_into_url: "http://localhost:5174/settings" } } }; };
    const view = await mountView(async () => new Response(JSON.stringify(merged())));
    const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.comment"), "the Comment button");
    expect(comment.disabled).toBe(true);
    (await import("./ui/more-menu.svelte")).loadMoreMenu();
    const link = await waitFor(() => Array.from(view.root.querySelectorAll<HTMLAnchorElement>("a")).find(a => a.textContent?.startsWith("Merged into")), "the Merged into link");
    expect(link.textContent).toBe("Merged into localhost:5174/settings");
    expect(link.getAttribute("href")).toBe("/a/8r4m0nzy3c5v");
    expect(Array.from(view.root.querySelectorAll("a")).some(a => a.textContent === "Open page" && a.classList.contains("hide-sm"))).toBe(false);
  });

  it("calls the versions snapshots in the version menu", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(liveArtifact())));
    const vbtn = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.vbtn"), "the version button");
    expect(vbtn.getAttribute("aria-label")).toBe("Snapshot 2 of 2");
    flush(() => vbtn.click());
    const rows = await waitFor(() => { const r = view.root.querySelectorAll(".vrow"); return r.length === 2 && r; }, "the version rows");
    expect(rows[0].querySelector(".h")?.textContent).toContain("snapshot");
    // The daemon's "snapshot" note would repeat the row's name: v1 reads First snapshot, v2 has no note.
    expect(rows[1].querySelector(".note")?.textContent).toBe("First snapshot");
    expect(rows[0].querySelector(".note")).toBeNull();
    expect(view.root.querySelector(".vmenu")?.textContent).not.toMatch(/publish|command line/);
  });

  it("keeps Comment enabled, with no hint and no Open page, on an artifact that is not live", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.comment"), "the Comment button");
    expect(comment.disabled).toBe(false);
    expect(comment.hasAttribute("title")).toBe(false);
    (await import("./ui/more-menu.svelte")).loadMoreMenu();
    await waitFor(() => view.root.querySelector("button.who"), "the roster");
    expect(Array.from(view.root.querySelectorAll("a")).some(a => a.textContent === "Open page")).toBe(false);
  });
});

describe("the top bar and the sidebar with the Comments switch, and an app's header", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();
  const APP = { version: 8, bundle_version: 3, installs: 12, published_by: { name: "Jun", login: "junp" }, owner: { name: "Alex", login: "akesling" } };
  const withFields = (extra: Record<string, unknown>) => { const a = artifact(8); return { ...a, artifact: { ...a.artifact, ...extra } }; };
  const answer = (extra: Record<string, unknown>) => async () => new Response(JSON.stringify(withFields(extra)));

  it("comment button disabled when comments off", async () => {
    const view = await mountView(answer({ comments: false }));
    const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.comment"), "the Comment button");
    expect(comment.disabled).toBe(true);
    expect(comment.title).toBe("Comments are off for this page");
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true }));
    expect(comment.getAttribute("aria-pressed")).toBe("false");
    expect(view.root.querySelector(".topbar.commenting")).toBeNull();
  });

  it("sidebar has no composer when comments off", async () => {
    stubMedia({ "(min-width: 900px)": true });
    const on = await mountView(answer({ comments: true }));
    await waitFor(() => on.root.querySelector(".new-comment-open"), "the New comment box while comments are on");
    on.unmount();
    const off = await mountView(answer({ comments: false }));
    await waitFor(() => off.root.querySelector("aside.sidebar:not([aria-busy])"), "the thread list");
    // Every chunk the sidebar asked for has loaded: a New comment box would be drawn by now.
    await vi.dynamicImportSettled();
    flush();
    expect(off.root.querySelector(".new-comment-open")).toBeNull();
    expect(off.root.querySelector("aside.sidebar .empty-open")!.textContent).toBe("Comments are off for this page.");
  });

  const thread = { id: "tA", artifact_id: ID, version_n: 8, anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
    status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
    comments: [{ id: "c1", thread_id: "tA", author_kind: "viewer", author_name: "Mia", via_harness: null, body: "Bigger buttons", created_at: "x" }] };
  /** The comment routes with one open thread, recording each write; a reply and a send are refused `comments_off`. */
  const threadRoutes = (writes: string[]) => async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      writes.push(url);
      if (url.endsWith("/comments") || url.endsWith("/send") || url.endsWith("threads:send")) return new Response(JSON.stringify({ error: { code: "comments_off", message: "comments are off on this artifact" } }), { status: 403 });
      return new Response(JSON.stringify({ thread: { ...thread, status: "resolved" } }));
    }
    return new Response(JSON.stringify(url.includes("/threads") ? { threads: [thread], next_cursor: null } : viewer));
  };

  it("with comments off, a thread keeps Resolve and Send but has no reply box", async () => {
    stubMedia({ "(min-width: 900px)": true });
    const writes: string[] = [];
    const view = await mountView(answer({ comments: false }), threadRoutes(writes));
    const card = await waitFor(() => view.root.querySelector<HTMLElement>('[data-thread="tA"]'), "the thread's card");
    flush(() => card.click());
    const resolve = await waitFor(() => Array.from(card.querySelectorAll("button")).find(b => b.textContent === "Resolve"), "Resolve");
    expect(Array.from(card.querySelectorAll("button")).some(b => b.textContent?.startsWith("Send to"))).toBe(true);
    expect(card.querySelector("form.reply")).toBeNull();
    expect(view.root.querySelector(".new-comment-open")).toBeNull();
    // The sidebar says why, above the open threads.
    expect(view.root.querySelector("aside.sidebar .section-open .no-reply")!.textContent).toBe("Comments are off for this page.");
    // A pointer's click: every load starts with the keyboard trail tainted (`keyboardTrail`).
    (await import("./view/trail")).keyboardTrail.clear();
    resolve.click();
    await waitFor(() => writes.some(u => u.endsWith("/threads/tA/resolve")), "the resolve request");
  });

  it("with comments off, a viewer who may not resolve gets no Send, no ticking and no Send unsent bar", async () => {
    stubMedia({ "(min-width: 900px)": true });
    const view = await mountView(answer({ comments: false, may_resolve: false }), threadRoutes([]));
    const card = await waitFor(() => view.root.querySelector<HTMLElement>('[data-thread="tA"]'), "the thread's card");
    flush(() => card.click());
    await waitFor(() => Array.from(card.querySelectorAll("button")).find(b => b.textContent === "Resolve"), "Resolve");
    expect(Array.from(card.querySelectorAll("button")).some(b => b.textContent?.startsWith("Send to"))).toBe(false);
    expect(card.querySelector('input[type="checkbox"]')).toBeNull();
    expect(view.root.querySelector(".send-unsent")).toBeNull();
    // Comments on again: Send is back.
    flush(() => view.controller().noteComments(true));
    await waitFor(() => Array.from(card.querySelectorAll("button")).find(b => b.textContent?.startsWith("Send to")), "Send");
    expect(view.root.querySelector(".send-unsent")).not.toBeNull();
  });

  it("a reply refused comments_off says so and turns comments off in the view", async () => {
    stubMedia({ "(min-width: 900px)": true });
    const writes: string[] = [];
    const view = await mountView(answer({ comments: true }), threadRoutes(writes));
    const comment = await waitFor(() => view.root.querySelector<HTMLButtonElement>("button.comment"), "the Comment button");
    await waitFor(() => view.root.querySelector('[data-thread="tA"]'), "the thread's card");
    expect(comment.disabled).toBe(false);
    view.controller().reply(thread as never, "one more");
    await waitFor(() => comment.disabled, "Comment turned off");
    expect(comment.title).toBe("Comments are off for this page");
    expect(writes.some(u => u.endsWith("/threads/tA/comments"))).toBe(true);
    const notice = await waitFor(() => view.root.querySelector(".banner.notice"), "the notice");
    expect(notice.textContent).toContain("Could not post: comments are off for this page");
  });

  it("a send refused comments_off (a commenter, while comments are off) says so", async () => {
    stubMedia({ "(min-width: 900px)": true });
    const writes: string[] = [];
    const view = await mountView(answer({ comments: false }), threadRoutes(writes));
    await waitFor(() => view.root.querySelector('[data-thread="tA"]'), "the thread's card");
    view.controller().sendThread(thread as never);
    const notice = await waitFor(() => view.root.querySelector(".banner.notice"), "the notice");
    expect(notice.textContent).toContain("Could not send to the agent: comments are off for this page");
    expect(writes.some(u => u.endsWith("/threads/tA/send"))).toBe(true);
    // The batch send of every unsent thread says the same.
    view.controller().dismissNotice();
    await view.controller().sendUnsent();
    await waitFor(() => writes.some(u => u.endsWith("threads:send")), "the batch send");
    await waitFor(() => view.root.querySelector(".banner.notice")?.textContent?.includes("Could not send to the agent: comments are off for this page"), "the batch's notice");
  });

  it("Get in Clax shows only for apps", async () => {
    const app = await mountView(answer({ app: APP, forks: 2 }));
    const get = await waitFor(() => app.root.querySelector<HTMLAnchorElement>("a.get-in-clax"), "Get in Clax");
    expect(get.textContent?.trim()).toBe("Get in Clax");
    expect(get.getAttribute("href")).toBe(`/a/${ID}/get`);
    // At phone width the bar hides it, and the more menu lists it.
    expect(get.classList.contains("hide-sm")).toBe(true);
    expect(app.root.querySelector(".by")?.textContent).toBe("v3 by @junp · by @akesling · 12 installs · 2 forks");
    app.unmount();
    const plain = await mountView(answer({ app: null }));
    await waitFor(() => plain.root.querySelector("button.comment"), "the Comment button");
    // The bar is drawn and every chunk it asked for has loaded.
    await vi.dynamicImportSettled();
    flush();
    expect(plain.root.querySelector("a.get-in-clax")).toBeNull();
  });

  it("Get in Clax is solid ink for a visitor and a ghost for whoever may share", () => {
    const visitor = mount(GetInClax, { id: ID });
    expect(visitor.root.querySelector("a.get-in-clax")!.classList.contains("ghost")).toBe(false);
    visitor.unmount();
    const sharer = mount(GetInClax, { id: ID, ghost: true });
    expect(sharer.root.querySelector("a.get-in-clax")!.classList.contains("ghost")).toBe(true);
    sharer.unmount();
  });
});
