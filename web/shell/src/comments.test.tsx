import { render } from "preact";
import { describe, expect, it, vi } from "vitest";
import { type Draft, PIN_RIGHT_ROOM, Pins, nextDraft, takePick, withClip } from "./comments";
import { Sidebar } from "./sidebar";
import { type Thread, areaLabel } from "./threads";

const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const thread = (id: string, status: "open" | "resolved" = "open"): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, anchor, status, sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "2026-09-29T10:00:00.000Z" }],
});
const at = (id: string, y: number) => ({ id, found: true, method: "exact" as const, rect: { x: 10, y, w: 100, h: 20 } });

function mount(node: preact.ComponentChild) {
  const root = document.createElement("div");
  document.body.appendChild(root);
  render(node, root);
  return { root, done: () => { render(null, root); root.remove(); } };
}

describe("Pins", () => {
  it("numbers pins like the sidebar's Open section and skips detached, resolved, unmeasured, and scrolled-away threads", () => {
    const threads = [thread("a"), thread("b"), thread("c"), thread("d"), thread("e", "resolved"), thread("f")];
    const resolved = { a: at("a", 40), b: { id: "b", found: false, method: null, rect: null }, c: at("c", -50), e: at("e", 40), f: at("f", 200) };
    const { root, done } = mount(<Pins threads={threads} resolved={resolved} onSelect={vi.fn()} />);
    // Open and not detached: a (1), c (2), d (3, not measured yet), f (4).
    expect(Array.from(root.querySelectorAll("button.thread-pin")).map(b => b.textContent)).toEqual(["1", "4"]);
    const { root: side, done: doneSide } = mount(<Sidebar threads={threads} resolved={resolved} now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />);
    expect(Array.from(side.querySelectorAll(".section-open .thread-num")).map(n => n.textContent)).toEqual(["1", "2", "3", "4"]);
    // A viewer comment must not carry the page-level `.viewer` layout class.
    expect(side.querySelector(".comment.viewer")).toBeNull();
    expect(side.querySelector(".comment.from-viewer .author")!.textContent).toBe("Viewer");
    done();
    doneSide();
  });

  it("pins only the threads on the page the frame shows, numbered like the sidebar", () => {
    const onAbout = (id: string): Thread => ({ ...thread(id), anchor: { ...anchor, file: "about.html" } });
    const threads = [thread("a"), onAbout("b"), onAbout("c")];
    const resolved = { a: at("a", 40), b: at("b", 80), c: at("c", 120) };
    const pins = (file: string) => {
      const { root, done } = mount(<Pins threads={threads} resolved={resolved} file={file} onSelect={vi.fn()} width={800} />);
      const out = Array.from(root.querySelectorAll<HTMLElement>("button.thread-pin")).map(b => `${b.textContent}@${b.style.top}`);
      done();
      return out;
    };
    expect(pins("index.html")).toEqual(["1@28px"]);
    expect(pins("about.html")).toEqual(["1@68px", "2@108px"]);
    const { root: side, done } = mount(<Sidebar threads={threads} resolved={resolved} file="about.html" now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />);
    expect(Array.from(side.querySelectorAll(".section-open .thread-num")).map(n => n.textContent)).toEqual(["1", "2"]);
    done();
  });

  it("pins an area thread at its drawn area's top right, labels it by its share, and reports hovers on pins and cards", () => {
    const area: Thread = { ...thread("a"), anchor: { ...anchor, kind: "area", selector: "main > section", quote: null, area: { x: 0.1, y: 0.2, w: 0.4213, h: 0.18 } } };
    const resolved = { a: { id: "a", found: true, method: "selector" as const, rect: { x: 100, y: 300, w: 200, h: 80 } } };
    const onHover = vi.fn();
    const { root, done } = mount(<Pins threads={[area]} resolved={resolved} onSelect={vi.fn()} onHover={onHover} width={800} />);
    const pin = root.querySelector<HTMLElement>("button.thread-pin")!;
    expect([pin.style.left, pin.style.top]).toEqual(["288px", "288px"]);
    pin.dispatchEvent(new MouseEvent("mouseenter"));
    expect(onHover).toHaveBeenLastCalledWith(area);
    pin.dispatchEvent(new MouseEvent("mouseleave"));
    expect(onHover).toHaveBeenLastCalledWith(null);
    done();
    const { root: side, done: doneSide } = mount(<Sidebar threads={[area]} resolved={resolved} now={new Date()} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} onHover={onHover} />);
    expect(side.querySelector(".anchor-label")!.textContent).toBe("Area in main > section (42% × 18%)");
    expect(areaLabel({ ...area.anchor, area: { x: 0, y: 0, w: 1, h: 0.004 } })).toBe("Area in main > section (100% × <1%)");
    const card = side.querySelector(".thread-card")!;
    onHover.mockClear();
    card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(onHover).toHaveBeenLastCalledWith(area);
    card.dispatchEvent(new MouseEvent("mouseleave"));
    expect(onHover).toHaveBeenLastCalledWith(null);
    doneSide();
  });

  it("keeps a full-width region's pin inside the stage and clear of the frame's scrollbar", () => {
    const wide = { a: { id: "a", found: true, method: "exact" as const, rect: { x: 0, y: 40, w: 400, h: 20 } } };
    const { root, done } = mount(<Pins threads={[thread("a")]} resolved={wide} onSelect={vi.fn()} width={400} />);
    expect(root.querySelector<HTMLElement>("button.thread-pin")!.style.left).toBe(`${400 - PIN_RIGHT_ROOM}px`);
    done();
  });
});

describe("page-opened composers", () => {
  const d = (quote: string): Omit<Draft, "pickId"> => ({ anchor: { ...anchor, quote }, version: 1, clip: null });
  it("open fresh, refuse over typed text, and move with the text for a drawn area", () => {
    const open: Draft = { pickId: "p1", ...d("first") };
    expect(nextDraft(null, "", d("a"), undefined, () => "n")).toMatchObject({ pickId: "n", anchor: { quote: "a" } });
    expect(nextDraft(open, "  ", d("a"), undefined, () => "n")).toMatchObject({ pickId: "n" });
    expect(nextDraft(open, "typed", d("a"))).toBeNull();
    // Same pickId: the composer (and its text) is kept, on the new anchor.
    expect(nextDraft(open, "typed", d("b"), { area: true })).toMatchObject({ pickId: "p1", anchor: { quote: "b" } });
  });
  it("take a pick only with its start, using the start up even when comment mode is off", () => {
    const started = new Map([["p1", 1], ["p2", 2]]);
    expect(takePick(started, "p1", false)).toBe(false);
    expect(started.has("p1")).toBe(false);
    expect(takePick(started, "p1", true)).toBe(false);
    expect(takePick(started, "nope", true)).toBe(false);
    expect(takePick(started, 5, true)).toBe(false);
    expect(takePick(started, "p2", true)).toBe(true);
    expect(started.size).toBe(0);
  });
  it("take a late clip only while the draft still waits for that token", () => {
    const waiting: Draft = { pickId: "p1", ...d("a"), capturing: true, clipToken: "t1" };
    const png = new Blob([new Uint8Array([1])]);
    expect(withClip(waiting, "t1", png)).toMatchObject({ clip: png, capturing: false, clipToken: undefined });
    expect(withClip(waiting, "t2", png)).toBe(waiting);
    expect(withClip(null, "t1", png)).toBeNull();
    const moved = { ...waiting, clipToken: "t3" };
    expect(withClip(moved, "t1", png)).toBe(moved);
  });
  it("say a screenshot is being taken", async () => {
    const { Composer } = await import("./comments");
    const { root, done } = mount(<Composer draft={{ pickId: "p", ...d("a"), capturing: true }} onCancel={vi.fn()} onSubmit={vi.fn(async () => {})} />);
    expect(root.textContent).toContain("Taking the screenshot…");
    done();
  });
});
