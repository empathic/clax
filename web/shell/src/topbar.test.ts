import { describe, expect, it, vi } from "vitest";
import { artifact, artifactViewHooks, mountView, waitFor } from "./test/artifact-view";
import { flush, mount } from "./test/svelte";
import MoreMenu from "./ui/MoreMenu.svelte";
import PhoneTabs from "./ui/PhoneTabs.svelte";

describe("Echo top bar parts", () => {
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

describe("the top bar on a live page", () => {
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
    await new Promise(r => setTimeout(r, 20));
    expect(comment.getAttribute("aria-pressed")).toBe("false");
    expect(view.root.querySelector(".topbar.commenting")).toBeNull();
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
