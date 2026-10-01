import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "./test/preact";
import { forgetViewer } from "./threads";
import { ViewerName } from "./viewer-name";

describe("ViewerName", () => {
  beforeEach(() => { forgetViewer(); });
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("sends a name save only after the initial lookup answered, so both use one viewer cookie", async () => {
    const calls: string[] = [];
    let answerGet!: () => void;
    vi.stubGlobal("fetch", vi.fn((_url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      calls.push(method);
      const res = (name: string | null) => new Response(JSON.stringify({ viewer: { public_id: "u_00000000000000000000aa", display_name: name, created_at: "x" } }));
      if (method === "GET") return new Promise<Response>(r => { answerGet = () => r(res(null)); });
      return Promise.resolve(res("Alex"));
    }));
    const view = mount(ViewerName, { setNotice: vi.fn() });
    const root = view.root;
    await new Promise(r => setTimeout(r, 0));
    const input = root.querySelector<HTMLInputElement>('input[aria-label="Your name"]')!;
    input.value = "Alex";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise(r => setTimeout(r, 0));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await new Promise(r => setTimeout(r, 20));
    expect(calls).toEqual(["GET"]);
    answerGet();
    await vi.waitFor(() => expect(calls).toEqual(["GET", "PUT"]));
    expect(input.value).toBe("Alex");
    view.unmount();
  });
});
