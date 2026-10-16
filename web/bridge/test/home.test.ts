import { describe, expect, it } from "vitest";
import { homeNamespace } from "../src/caps/home";
import { Rpc } from "../src/rpc";

describe("home in the page", () => {
  it("relays its calls to the shell, and on() hears the shell's home events until stopped", () => {
    const posted: unknown[] = [];
    const rpc = new Rpc(m => posted.push(m));
    rpc.connect();
    const home = homeNamespace(rpc) as {
      apps(): Promise<unknown>;
      menu(id: string, r: unknown): Promise<unknown>;
      on(name: string, fn: (v: unknown) => void): () => void;
    };
    void home.apps();
    void home.menu("a1", { x: 1, y: 2, w: 3, h: 4 });
    expect(posted).toMatchObject([
      { type: "clax:call", ns: "home", method: "apps", args: [] },
      { type: "clax:call", ns: "home", method: "menu", args: ["a1", { x: 1, y: 2, w: 3, h: 4 }] },
    ]);
    const got: unknown[] = [];
    const stop = home.on("unread", v => got.push(v));
    rpc.accept({ type: "clax:event", ns: "home", topic: "unread", data: 2 });
    rpc.accept({ type: "clax:event", ns: "home", topic: "apps", data: [] });
    stop();
    rpc.accept({ type: "clax:event", ns: "home", topic: "unread", data: 5 });
    expect(got).toEqual([2]);
    expect(Object.isFrozen(home)).toBe(true);
  });
});
