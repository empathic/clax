import { describe, expect, it, vi } from "vitest";
import { type ClaxPlugin, PLUGIN_API_VERSION, PluginHost, definePlugin } from "./plugin";

const plugin = (over: Partial<ClaxPlugin> = {}): ClaxPlugin => ({ name: "p", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [], contributes: {}, ...over });
const png = (s: string) => Object.assign(new Blob([s], { type: "image/png" }), { tag: s });
const tag = (b: Blob) => (b as Blob & { tag: string }).tag;

describe("definePlugin", () => {
  it("takes a well-formed plugin as it is", () => {
    const p = plugin({ name: "acme/logs" });
    expect(definePlugin(p)).toBe(p);
  });
  it.each([
    [{ name: "Code Blur" }, /lower-case kebab/],
    [{ version: "" }, /version/],
    [{ apiVersion: 1.5 }, /apiVersion/],
    [{ capabilities: ["net:anywhere" as never] }, /no capability/],
    [{ contributes: { toolbar: [] } as never }, /no slot/],
  ])("refuses %o", (over, msg) => {
    expect(() => definePlugin(plugin(over))).toThrow(msg);
  });
});

describe("PluginHost", () => {
  it("refuses another major of the contract, an ungranted capability, a second plugin of a name, and a setup that throws", () => {
    const host = new PluginHost([
      plugin({ name: "old", apiVersion: 2 }),
      plugin({ name: "net", capabilities: ["net:host"] }),
      plugin({ name: "ok" }),
      plugin({ name: "ok" }),
      plugin({ name: "boom", setup: () => { throw new Error("no"); } }),
    ], ["dom:read"]);
    expect(host.plugins.map(p => p.name)).toEqual(["ok"]);
    expect(host.refused.map(r => r.name)).toEqual(["old", "net", "ok", "boom"]);
    expect(host.refused[0].reason).toMatch(/plugin API 2/);
    expect(host.refused[1].reason).toMatch(/net:host, not granted/);
  });

  it("lists contributions in plugin order and each annotate tool once", () => {
    const host = new PluginHost([
      plugin({ name: "a", contributes: { pickers: [{ id: "element", label: "Element" }], annotate: { tools: [{ id: "box", label: "Box" }] } } }),
      plugin({ name: "b", contributes: { pickers: [{ id: "area", label: "Area" }], annotate: { tools: [{ id: "box", label: "Box" }, { id: "highlight", label: "Highlight" }] } } }),
    ]);
    expect(host.list("pickers").map(p => p.id)).toEqual(["element", "area"]);
    expect(host.tools().map(t => t.id)).toEqual(["box", "highlight"]);
    expect(host.list("filters")).toEqual([]);
  });

  it("keeps only --clax-* theme tokens, later over earlier", () => {
    const host = new PluginHost([
      plugin({ name: "a", contributes: { theme: { tokens: { "--clax-bg": "#fff", "--clax-fg": "#000" } } } }),
      plugin({ name: "b", contributes: { theme: { tokens: { "--clax-bg": "#111", "--bg": "red", "color": "blue" } } } }),
    ]);
    expect(host.theme()).toEqual({ "--clax-bg": "#111", "--clax-fg": "#000" });
  });

  it("picks a presentation, following fallbacks when one is not available, without looping", () => {
    const host = new PluginHost([plugin({ name: "pres", contributes: { presentations: [
      { id: "floating", label: "Floating", layout: "dock" },
      { id: "dialog", label: "Dialog", layout: "dialog" },
      { id: "extension", label: "Extension", layout: "dialog", available: () => false, fallback: "dialog" },
      { id: "loop", label: "Loop", layout: "dock", available: () => false, fallback: "loop" },
    ] } })]);
    expect(host.presentation("extension", window)?.id).toBe("dialog");
    expect(host.presentation("loop", window)?.id).toBe("floating");
    expect(host.presentation(undefined, window)?.id).toBe("floating");
    expect(new PluginHost().presentation("dialog", window)).toBeNull();
  });

  it("runs the filters that are on, in order, and stops the capture when one fails", async () => {
    const seen: string[] = [];
    const host = new PluginHost([plugin({ name: "f", contributes: { filters: [
      { id: "one", label: "One", default: true, apply: async i => { seen.push(tag(i.png)); return { png: png("1"), note: "one ran" }; } },
      { id: "off", label: "Off", default: false, apply: async () => { throw new Error("never"); } },
      { id: "two", label: "Two", default: true, apply: async i => { seen.push(tag(i.png)); return { png: i.png, note: null }; } },
    ] } })]);
    const input = { png: png("0"), at: { x: 0, y: 0, w: 1, h: 1 }, scale: 1, doc: document };
    const out = await host.filter(input, ["one", "two"]);
    expect(seen).toEqual(["0", "1"]);
    expect(tag(out.png)).toBe("1");
    expect(out.notes).toEqual([{ id: "one", note: "one ran" }]);
    await expect(host.filter(input, ["off"])).rejects.toThrow(/Off filter could not run.*left out/);
  });

  it("sends an attachment's data only with its sent-row, and merges what the ticked ones give", async () => {
    const row = (id: string) => ({ id, label: id, preview: "", optIn: true, default: false });
    const host = new PluginHost([plugin({ name: "a", contributes: { attachments: [
      { id: "logs", label: "Logs", default: false, collect: () => ({ data: { diagnostics: [{ at: "t", level: "warn", kind: "console", text: "x" }] }, row: row("logs") }) },
      { id: "version", label: "Version", default: true, collect: () => ({ data: { release: { version: "1.2" }, app: { a: 1 } }, row: row("version") }) },
      { id: "sneaky", label: "Sneaky", default: true, collect: () => ({ data: { app: { secret: 1 } }, row: null }) },
      { id: "broken", label: "Broken", default: true, collect: () => { throw new Error("down"); } },
    ] } })]);
    const out = await host.attach(["logs", "version", "sneaky", "broken"], document);
    expect(out.rows.map(r => r.id)).toEqual(["logs", "version"]);
    expect(out.data).toEqual({ diagnostics: [{ at: "t", level: "warn", kind: "console", text: "x" }], release: { version: "1.2" }, app: { a: 1 } });
    expect(out.refused).toEqual([{ name: "sneaky", reason: expect.stringMatching(/no sent-row/) }, { name: "broken", reason: "failed: down" }]);
    expect((await host.attach(["version"], document)).rows.map(r => r.id)).toEqual(["version"]);
  });

  it("merges the pickers' refinements and skips one that throws", () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    const host = new PluginHost([plugin({ name: "pk", contributes: { pickers: [
      { id: "code-line", label: "Code line", refine: () => ({ code: { file: "a.ts" } }) },
      { id: "bad", label: "Bad", refine: () => { throw new Error("x"); } },
      { id: "element", label: "Element", mode: "element" },
    ] } })]);
    expect(host.refine(null, document)).toEqual({ code: { file: "a.ts" } });
    expect(err).toHaveBeenCalledOnce();
    err.mockRestore();
  });

  it("runs the bus: a cancellable event keeps the first reason, others cannot be cancelled, setup listens and tears down", () => {
    const off = vi.fn();
    const heard: string[] = [];
    const host = new PluginHost([plugin({ name: "veto", setup: ctx => {
      ctx.on("before-send", m => { heard.push(m.body); m.cancel("no logs on Fridays"); });
      ctx.on("sent", m => { heard.push(m.id); m.cancel("too late"); });
      return off;
    } })]);
    host.on("before-send", m => m.cancel("second"));
    expect(host.emit("before-send", { body: "hi", rows: [] })).toEqual({ cancelled: "no logs on Fridays" });
    expect(host.emit("sent", { id: "r1" })).toEqual({ cancelled: null });
    expect(heard).toEqual(["hi", "r1"]);
    host.destroy();
    expect(off).toHaveBeenCalledOnce();
    expect(host.emit("before-send", { body: "x", rows: [] })).toEqual({ cancelled: null });
  });
});
