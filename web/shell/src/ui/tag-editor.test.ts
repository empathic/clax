import { afterEach, describe, expect, it, vi } from "vitest";
import { writable } from "svelte/store";
import { flush } from "../test/svelte";

let answer: ((r: Response) => void) | null = null;
const asked: string[] = [];

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const ctl = () => ({ id: "7q3k9mzx2b4t", disposed: false, state: writable({ tags: { tags: [], may: true } }), tagTicket: () => 0, setTags: () => {} });

describe("the tag editor", () => {
  it("opens on the press, saying it is reading the tags while their answer is on its way, and the next opening draws the list it read", async () => {
    vi.stubGlobal("fetch", vi.fn((url: string) => { asked.push(String(url)); return new Promise<Response>(r => { answer = r; }); }));
    const { toggleTagEditor } = await import("./tag-editor");
    const button = document.createElement("button");
    document.body.append(button);
    const c = ctl();
    flush(() => toggleTagEditor(button, c as never));
    // Open at once, the read still on its way.
    const pop = document.querySelector(".tag-pop")!;
    expect(pop).not.toBeNull();
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(pop.querySelector("[role=status]")?.textContent).toBe("Loading…");
    answer!(new Response(JSON.stringify({ tags: [{ id: "t1", name: "design", colour: "sky", description: null, created_at: "x", updated_at: "x", count: 1 }], colours: ["sky"], may_manage: false })));
    await vi.waitFor(() => expect(pop.textContent).toContain("design"));
    // A second press closes it.
    flush(() => toggleTagEditor(button, c as never));
    expect(document.querySelector(".tag-pop")).toBeNull();
    // Opened again: the list it read is drawn at once, and read again.
    flush(() => toggleTagEditor(button, c as never));
    expect(document.querySelector(".tag-pop")!.textContent).toContain("design");
    expect(document.querySelector(".tag-pop [role=status]")).toBeNull();
    expect(asked.filter(u => u === "/api/tags")).toHaveLength(2);
  });
});
