import { render } from "preact";
import { describe, expect, it, vi } from "vitest";
import { Sidebar } from "./sidebar";
import type { Thread } from "./threads";

const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-09-29T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const comment = (id: string, kind: "viewer" | "agent", name: string, body: string) => ({ id, thread_id: "t", author_kind: kind, author_name: name, via_harness: kind === "agent" ? name : null, body, created_at: base.created_at });

describe("Sidebar", () => {
  it("groups open, detached, and resolved threads and labels agent comments", () => {
    const threads: Thread[] = [
      { ...base, id: "a", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns"), comment("2", "agent", "claude", "done")],
        feedback_state: { thread_id: "a", state: "acknowledged", tier: "wait", since: base.created_at, resends: 0, exhausted: false } },
      { ...base, id: "b", anchor, status: "open", sent_to_agent: false, comments: [comment("3", "viewer", "Viewer", "gone")] },
      { ...base, id: "c", anchor, status: "resolved", sent_to_agent: false, comments: [comment("4", "viewer", "Viewer", "old")] },
    ];
    const found = { a: { id: "a", found: true, method: "exact" as const, rect: null }, b: { id: "b", found: false, method: null, rect: null } };
    const root = document.createElement("div");
    document.body.appendChild(root);
    render(<Sidebar threads={threads} resolved={found} now={new Date(base.created_at)} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />, root);
    expect(root.querySelectorAll(".section-open .thread-card")).toHaveLength(1);
    expect(root.querySelectorAll(".section-detached .thread-card")).toHaveLength(1);
    expect(root.querySelectorAll(".section-resolved .thread-card")).toHaveLength(1);
    expect(root.querySelector(".comment.agent .author")!.textContent).toBe("Agent · via claude");
    expect(root.querySelector(".section-open .waiting")!.textContent).toBe("seen by the agent");
    // Only thread b is open and unsent; a was sent and c is resolved.
    expect(Array.from(root.querySelectorAll("button")).filter(b => b.textContent === "Send to agent")).toHaveLength(1);
    render(null, root);
    root.remove();
  });

  it("names who resolved a thread without exposing identifiers", () => {
    const me = { public_id: "u_0123456789abcdef012345", display_name: "Alex", created_at: base.created_at };
    const resolved = (id: string, by: string): Thread => ({ ...base, id, anchor, status: "resolved", sent_to_agent: false, resolved_at: base.created_at, resolved_by: by, comments: [comment(id, "viewer", "Viewer", "x")] });
    const threads = [
      resolved("a", "viewer:u_0123456789abcdef012345"),
      resolved("b", "viewer:u_ffffffffffffffffffffff"),
      resolved("c", "viewer:anonymous"),
      resolved("d", "agent:codex"),
    ];
    const root = document.createElement("div");
    document.body.appendChild(root);
    render(<Sidebar threads={threads} resolved={{}} me={me} now={new Date(base.created_at)} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />, root);
    expect(Array.from(root.querySelectorAll(".resolved-by")).map(e => e.textContent)).toEqual([
      "Resolved by Alex", "Resolved by Viewer", "Resolved by Viewer", "Resolved by Agent · via codex",
    ]);
    render(null, root);
    root.remove();
  });

  it("labels threads on other pages and never counts them as detached here", () => {
    const onAbout = { ...anchor, file: "about.html" };
    const threads: Thread[] = [
      { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "here")] },
      { ...base, id: "b", anchor: onAbout, status: "open", sent_to_agent: false, comments: [comment("2", "viewer", "Alex", "there")] },
      { ...base, id: "c", anchor: onAbout, status: "resolved", sent_to_agent: false, comments: [comment("3", "viewer", "Alex", "done there")] },
    ];
    // A stale result from another page must not detach b.
    const found = { a: { id: "a", found: false, method: null, rect: null }, b: { id: "b", found: false, method: null, rect: null } };
    const root = document.createElement("div");
    document.body.appendChild(root);
    render(<Sidebar threads={threads} resolved={found} file="index.html" now={new Date(base.created_at)} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />, root);
    expect(Array.from(root.querySelectorAll(".section-detached .thread-card")).map(c => c.getAttribute("data-thread"))).toEqual(["a"]);
    const open = Array.from(root.querySelectorAll<HTMLElement>(".section-open .thread-card"));
    expect(open.map(c => c.getAttribute("data-thread"))).toEqual(["b"]);
    expect(open[0].querySelector(".thread-num")).toBeNull();
    expect(open[0].querySelector(".file-label")!.textContent).toBe("on about.html");
    expect(root.querySelector('[data-thread="c"] .file-label')!.textContent).toBe("on about.html");
    expect(root.querySelector('[data-thread="a"] .file-label')).toBeNull();
    render(null, root);
    root.remove();
  });

  it("selects a thread from its keyboard-reachable header button", () => {

    const onSelect = vi.fn();
    const t: Thread = { ...base, id: "a", anchor, status: "open", sent_to_agent: false, comments: [comment("1", "viewer", "Alex", "note")] };
    const root = document.createElement("div");
    document.body.appendChild(root);
    render(<Sidebar threads={[t]} resolved={{}} now={new Date(base.created_at)} selected={null}
      onSelect={onSelect} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />, root);
    const head = root.querySelector<HTMLButtonElement>(".thread-card button.card-head")!;
    expect(head.type).toBe("button");
    head.click();
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith(t);
    render(null, root);
    root.remove();
  });
});
