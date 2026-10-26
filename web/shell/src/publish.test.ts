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

/** Stubs fetch for a local Clax signed in nowhere: `POST /api/remotes`
 * starts a sign-in to clax.dev, and its first poll answers `status`; once
 * connected, the artifact's remotes list `connected`. Records each start's
 * body and each poll's remote. */
function signInDaemon(connected: unknown, status: string) {
  const calls = { added: [] as unknown[], polled: [] as string[] };
  let remotes: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/api/token") return new Response(JSON.stringify({ token: "t-local" }));
    if (url === "/api/artifacts/a1/remotes") return new Response(JSON.stringify({ links: [], remotes }));
    if (url === "/api/remotes" && init?.method === "POST") {
      calls.added.push(JSON.parse(String(init.body)));
      return new Response(JSON.stringify({
        remote: { id: "r_2", origin: "https://clax.dev", name: "clax.dev", status: "pending", person_name: null },
        user_code: "ABCD-1234", verification_url: "https://clax.dev/device?code=ABCD-1234", expires_at: "2026-10-26T19:00:00Z", interval: 5,
      }), { status: 201 });
    }
    const poll = /^\/api\/remotes\/([^/]+)\/poll$/.exec(url);
    if (poll && init?.method === "POST") {
      calls.polled.push(poll[1]);
      if (status === "connected") remotes = [connected];
      return new Response(JSON.stringify({ remote: connected, status }));
    }
    return new Response("{}", { status: 404 });
  }));
  return calls;
}

const settle = async () => { for (let i = 0; i < 5; i++) { await new Promise(r => setTimeout(r)); flush(); } };
const remote = { id: "r_1", origin: "https://clax.example", name: "clax.example", status: "connected", person_name: "Ada" };

describe("the Publish button", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("with no public Clax, offers Publish to clax.dev, signs this Clax in there first, then asks to publish", async () => {
    const dev = { id: "r_2", origin: "https://clax.dev", name: "clax.dev", status: "connected", person_name: "Ada" };
    const calls = signInDaemon(dev, "connected");
    const tab = { location: { href: "" }, opener: {} as unknown };
    const opened = vi.fn(() => tab);
    vi.stubGlobal("open", opened);
    const m = mount(Publish, { id: "a1", title: "Plan", latest: 2 });
    await settle();
    const b = m.root.querySelector<HTMLButtonElement>("button.publish")!;
    expect(b.textContent).toBe("Publish to clax.dev");
    flush(() => b.click());
    const pop = m.root.querySelector("[role=dialog]")!;
    expect(pop.textContent).toContain("sign this Clax in to clax.dev");
    flush(() => pop.querySelector<HTMLButtonElement>("button.primary")!.click());
    await settle();
    expect(calls.added).toEqual([{ url: "https://clax.dev" }]);
    expect(opened).toHaveBeenCalledTimes(1);
    expect(tab.location.href).toBe("https://clax.dev/device?code=ABCD-1234");
    expect(tab.opener).toBeNull();
    expect(calls.polled).toEqual(["r_2"]);
    expect(m.root.querySelector("[role=dialog]")!.textContent).toContain("Publish “Plan” on clax.dev, as Ada");
    m.unmount();
  });

  it("shows the code while the sign-in waits, and says so when it is denied", async () => {
    signInDaemon(null, "denied");
    vi.stubGlobal("open", vi.fn(() => null));
    const m = mount(Publish, { id: "a1", title: "Plan", latest: 1 });
    await settle();
    flush(() => m.root.querySelector<HTMLButtonElement>("button.publish")!.click());
    flush(() => m.root.querySelector<HTMLButtonElement>("[role=dialog] button.primary")!.click());
    await settle();
    const pop = m.root.querySelector("[role=dialog]")!;
    expect(pop.querySelector("[role=alert]")!.textContent).toContain("was declined");
    expect(pop.querySelector<HTMLButtonElement>("button.primary")!.textContent).toBe("Sign in to clax.dev");
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
