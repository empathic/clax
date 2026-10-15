import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import Publish from "./ui/Publish.svelte";

type Reply = [number, unknown];

/** Stubs fetch: the owner's token, `remotes` for the artifact's remotes,
 * and `pushed` for a push; records each push's body. */
function daemon(remotes: unknown, pushed: Reply = [200, {}]) {
  const pushes: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/api/token") return new Response(JSON.stringify({ token: "t-local" }));
    if (url.endsWith("/remotes")) return new Response(JSON.stringify(remotes));
    if (url.endsWith("/push")) {
      pushes.push(JSON.parse(String(init?.body)));
      return new Response(JSON.stringify(pushed[1]), { status: pushed[0] });
    }
    return new Response("{}", { status: 404 });
  }));
  return pushes;
}

const settle = async () => { for (let i = 0; i < 5; i++) { await new Promise(r => setTimeout(r)); flush(); } };
const remote = { id: "r_1", origin: "https://clax.example", name: "clax.example", status: "connected", person_name: "Ada" };

describe("the Publish button", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("shows nothing when this Clax is signed in to no public Clax", async () => {
    daemon({ links: [], remotes: [] });
    const m = mount(Publish, { id: "a1", title: "Plan", latest: 2 });
    await settle();
    expect(m.root.querySelector("button, a")).toBeNull();
    m.unmount();
  });

  it("publishes to the remote once confirmed, with the owner's follow choice, and links the public copy", async () => {
    const pushes = daemon({ links: [], remotes: [remote] }, [200, {
      status: "pushed", url: "https://clax.example/a/b1", assets_not_pushed: 0,
      link: { remote_id: "r_1", url: "https://clax.example/a/b1", pushed_n: 2, remote_n: 2, auto: true, behind: 0 },
      pushed: [{ n: 1, remote_n: 1 }, { n: 2, remote_n: 2 }],
    }]);
    const m = mount(Publish, { id: "a1", title: "Plan", latest: 2 });
    await settle();
    const b = m.root.querySelector<HTMLButtonElement>("button.publish")!;
    expect(b.textContent).toBe("Publish to clax.example");
    flush(() => b.click());
    const pop = m.root.querySelector("[role=dialog]")!;
    expect(pop.textContent).toContain("Publish “Plan” on clax.example, as Ada");
    flush(() => pop.querySelector<HTMLInputElement>("input[type=checkbox]")!.click());
    flush(() => pop.querySelector<HTMLButtonElement>("button.primary")!.click());
    await settle();
    expect(pushes).toEqual([{ remote: "r_1", mode: "update", auto: true }]);
    expect(m.root.querySelector("[role=dialog]")!.textContent).toContain("Pushed 2 versions to clax.example");
    m.unmount();
  });

  it("offers Push v<N> when behind, and a fork or a new artifact when the public copy moved", async () => {
    const pushes = daemon({
      links: [{ remote_id: "r_1", remote_name: "clax.example", origin: "https://clax.example", url: "https://clax.example/a/b1", pushed_n: 2, remote_n: 2, auto: false, behind: 1 }],
      remotes: [remote],
    }, [409, { error: { code: "remote_moved", message: "the public copy is at version 3" } }]);
    const m = mount(Publish, { id: "a1", title: "Plan", latest: 3 });
    await settle();
    expect(m.root.querySelector<HTMLAnchorElement>("a.public")!.href).toBe("https://clax.example/a/b1");
    const b = m.root.querySelector<HTMLButtonElement>("button.publish")!;
    expect(b.textContent).toBe("Push v3");
    flush(() => b.click());
    flush(() => m.root.querySelector<HTMLButtonElement>("[role=dialog] button.primary")!.click());
    await settle();
    const pop = m.root.querySelector("[role=dialog]")!;
    expect(pop.querySelector("[role=alert]")!.textContent).toContain("version 3");
    flush(() => pop.querySelector<HTMLButtonElement>("button.primary")!.click());
    await settle();
    expect(pushes).toEqual([{ remote: "r_1", mode: "update", auto: false }, { remote: "r_1", mode: "fork", auto: false }]);
    m.unmount();
  });
});
