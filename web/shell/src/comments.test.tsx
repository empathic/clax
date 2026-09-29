import { render } from "preact";
import { describe, expect, it, vi } from "vitest";
import { PIN_RIGHT_ROOM, Pins } from "./comments";
import { Sidebar } from "./sidebar";
import type { Thread } from "./threads";

const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null };
const thread = (id: string, status: "open" | "resolved" = "open"): Thread => ({
  id, artifact_id: "7q3k9mzx2b4t", version_n: 1, anchor, status, sent_to_agent: false, has_clip: false, clip_url: null,
  created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_session_id: null, body: `note ${id}`, created_at: "2026-09-29T10:00:00.000Z" }],
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

  it("keeps a full-width region's pin inside the stage and clear of the frame's scrollbar", () => {
    const wide = { a: { id: "a", found: true, method: "exact" as const, rect: { x: 0, y: 40, w: 400, h: 20 } } };
    const { root, done } = mount(<Pins threads={[thread("a")]} resolved={wide} onSelect={vi.fn()} width={400} />);
    expect(root.querySelector<HTMLElement>("button.thread-pin")!.style.left).toBe(`${400 - PIN_RIGHT_ROOM}px`);
    done();
  });
});
