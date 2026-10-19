import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { type DeviceCall, deviceLink } from "./device-link";
import { CapError } from "./errors";
import { CapabilityHost, type CapEnv } from "./host";
import { REGISTRY } from "./registry";

/** Whether the viewer's gesture in the page holds, as `frameGesture` says. */
let gesture = true;
vi.mock("./gesture", async orig => ({ ...(await orig<typeof import("./gesture")>()), frameGesture: () => gesture }));

function env(declared: CapEnv["declared"]) {
  const posted: ShellToBridge[] = [];
  const e: CapEnv = {
    aid: "7q3k9mzx2b4t", version: 1, pinned: false, token: "t",
    viewer: async () => ({ publicId: "u_00000000000000000000aa", name: "Alex" }),
    declared,
    prompt: vi.fn(async () => "allow" as const),
    post: m => posted.push(m),
    reload: vi.fn(),
  };
  return { e, posted };
}

afterEach(() => { deviceLink.call = null; deviceLink.home = undefined; deviceLink.events.clear(); gesture = true; });

/** Links a fake app that says whether this view is its home app (`home`),
 * and keeps every call with its answer's resolve and reject. */
function link(home: boolean) {
  const sent: (Omit<DeviceCall, "id"> & { resolve(v: unknown): void; reject(e: unknown): void })[] = [];
  deviceLink.call = (cap, method, args) => new Promise((resolve, reject) => { sent.push({ cap, method, args, resolve, reject }); });
  deviceLink.home = () => Promise.resolve(home);
  return sent;
}

const granted = (m: ShellToBridge | undefined) => (m as { granted?: boolean } | undefined)?.granted;

describe("the home capability", () => {
  it("is unavailable outside app mode, even when declared", async () => {
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    await host.handle({ type: "clax:use", id: "u1", name: "home" });
    await host.handle({ type: "clax:call", id: "c1", ns: "home", method: "apps", args: [] });
    expect(granted(posted[0])).toBe(false);
    expect(posted[1]).toMatchObject({ ok: false, error: { code: "not_granted" } });
  });

  it("in app mode, is available only when the app says this view is its home app", async () => {
    link(false);
    const other = env({ home: {} });
    await new CapabilityHost(Promise.resolve(other.e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "home" });
    expect(granted(other.posted[0])).toBe(false);

    link(true);
    const home = env({ home: {} });
    await new CapabilityHost(Promise.resolve(home.e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "home" });
    expect(granted(home.posted[0])).toBe(true);

    const undeclared = env({ db: {} });
    await new CapabilityHost(Promise.resolve(undeclared.e), REGISTRY, null).handle({ type: "clax:use", id: "u", name: "home" });
    expect(granted(undeclared.posted[0])).toBe(false);
  });

  it("relays calls as device {cap: home}, with positional arguments as one object", async () => {
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const apps = host.handle({ type: "clax:call", id: "c1", ns: "home", method: "apps", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "home", method: "apps", args: [] });
    sent[0].resolve([{ id: "a1" }]);
    await apps;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: true, value: [{ id: "a1" }] });

    void host.handle({ type: "clax:call", id: "c2", ns: "home", method: "open", args: ["a1"] });
    void host.handle({ type: "clax:call", id: "c3", ns: "home", method: "menu", args: ["a1", { x: 1, y: 2, w: 3, h: 4 }] });
    await vi.waitFor(() => expect(sent).toHaveLength(3));
    expect(sent[1]).toMatchObject({ cap: "home", method: "open", args: [{ id: "a1" }] });
    expect(sent[2]).toMatchObject({ cap: "home", method: "menu", args: [{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 } }] });
  });

  it("menu forwards the page's items, and sends {id, rect} alone without them", async () => {
    const sent = link(true);
    const { e } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const rect = { x: 1, y: 2, w: 3, h: 4 };
    void host.handle({ type: "clax:call", id: "m1", ns: "home", method: "menu", args: ["a1", rect, { items: [{ id: "star", label: " Star " }, { id: "rm", label: "Remove from group" }] }] });
    void host.handle({ type: "clax:call", id: "m2", ns: "home", method: "menu", args: ["a1", rect] });
    void host.handle({ type: "clax:call", id: "m3", ns: "home", method: "menu", args: ["a1", rect, {}] });
    void host.handle({ type: "clax:call", id: "m4", ns: "home", method: "menu", args: ["a1", rect, { items: [{ id: "a", label: "x".repeat(32) }] }] });
    await vi.waitFor(() => expect(sent.filter(s => s.method === "menu")).toHaveLength(4));
    const menus = sent.filter(s => s.method === "menu");
    expect(menus[0].args).toEqual([{ id: "a1", rect, items: [{ id: "star", label: "Star" }, { id: "rm", label: "Remove from group" }] }]);
    expect(menus[1].args).toEqual([{ id: "a1", rect }]);
    expect(menus[2].args).toEqual([{ id: "a1", rect }]);
    expect(menus[3].args).toEqual([{ id: "a1", rect, items: [{ id: "a", label: "x".repeat(32) }] }]);
  });

  it("menu refuses bad items with bad_request and sends nothing", async () => {
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const rect = { x: 1, y: 2, w: 3, h: 4 };
    const five = Array.from({ length: 5 }, (_, i) => ({ id: `i${i}`, label: "x" }));
    for (const options of [
      { items: five },
      { items: [{ id: "a", label: "x".repeat(33) }] },
      { items: [{ id: "a", label: "   " }] },
      { items: [{ id: "", label: "Star" }] },
      { items: [{ id: "a".repeat(65), label: "Star" }] },
      { items: [{ id: "a", label: "Star" }, { id: "a", label: "Unstar" }] },
      { items: [{ id: "a", label: "Line\nbreak" }] },
      { items: [{ id: 3, label: "Star" }] },
      { items: "star" },
      "star",
    ]) {
      await host.handle({ type: "clax:call", id: "b", ns: "home", method: "menu", args: ["a1", rect, options] });
      expect(posted.at(-1), JSON.stringify(options)).toMatchObject({ ok: false, error: { code: "bad_request" } });
    }
    expect(sent).toHaveLength(0);
  });

  it("passes the app's refusal on, and asks nothing for a bad call or one without a gesture", async () => {
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const done = host.handle({ type: "clax:call", id: "c1", ns: "home", method: "unread", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    sent[0].reject(new CapError("refused", "not the home app"));
    await done;
    expect(posted.at(-1)).toMatchObject({ ok: false, error: { code: "refused" } });

    await host.handle({ type: "clax:call", id: "c2", ns: "home", method: "menu", args: ["a1", { x: 1 }] });
    expect(posted.at(-1)).toMatchObject({ ok: false, error: { code: "bad_request" } });
    await host.handle({ type: "clax:call", id: "c3", ns: "home", method: "reboot", args: [] });
    expect(posted.at(-1)).toMatchObject({ ok: false, error: { code: "capability_removed" } });
    gesture = false;
    await host.handle({ type: "clax:call", id: "c4", ns: "home", method: "newApp", args: [] });
    expect(posted.at(-1)).toMatchObject({ ok: false, error: { code: "no_gesture" } });
    expect(sent).toHaveLength(1);
  });

  it("relays tiles without a gesture as {tiles: [{id, rect}]}, and refuses a malformed list", async () => {
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    gesture = false;
    void host.handle({ type: "clax:call", id: "c1", ns: "home", method: "tiles", args: [[{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 }, extra: true }]] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "home", method: "tiles", args: [{ tiles: [{ id: "a1", rect: { x: 1, y: 2, w: 3, h: 4 } }] }] });

    for (const bad of [[{ id: "a1", rect: { x: 1 } }], [{ rect: { x: 1, y: 2, w: 3, h: 4 } }], "a1", Array(513).fill({ id: "a", rect: { x: 0, y: 0, w: 1, h: 1 } })]) {
      await host.handle({ type: "clax:call", id: "c2", ns: "home", method: "tiles", args: [bad] });
      expect(posted.at(-1)).toMatchObject({ ok: false, error: { code: "bad_request" } });
    }
    expect(sent).toHaveLength(1);
  });

  it("passes the app's home events to the page as home/<name>", async () => {
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const done = host.handle({ type: "clax:call", id: "c1", ns: "home", method: "unread", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    sent[0].resolve(0);
    await done;
    for (const f of [...deviceLink.events]) {
      f({ cap: "home", name: "unread", data: 3 });
      f({ cap: "home", name: "secrets", data: 1 });
      f({ cap: "location", watch: "w", kind: "update", value: {} });
    }
    expect(posted.filter(m => m.type === "clax:event")).toEqual([{ type: "clax:event", ns: "home", topic: "unread", data: 3 }]);
  });

  it("relays chrome() without a gesture and returns the app's answer, and passes chrome events on", async () => {
    gesture = false;
    const sent = link(true);
    const { e, posted } = env({ home: {} });
    const host = new CapabilityHost(Promise.resolve(e), REGISTRY, null);
    const chrome = { platform: "android", form: "wide", toolbar: false, pointer: "mouse" };
    const done = host.handle({ type: "clax:call", id: "c1", ns: "home", method: "chrome", args: [] });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ cap: "home", method: "chrome", args: [] });
    sent[0].resolve(chrome);
    await done;
    expect(posted.at(-1)).toEqual({ type: "clax:call-result", id: "c1", ok: true, value: chrome });
    for (const f of [...deviceLink.events]) f({ cap: "home", name: "chrome", data: { ...chrome, pointer: "touch" } });
    expect(posted.filter(m => m.type === "clax:event")).toEqual([{ type: "clax:event", ns: "home", topic: "chrome", data: { ...chrome, pointer: "touch" } }]);
  });
});
