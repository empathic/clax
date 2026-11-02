import { afterEach, describe, expect, it } from "vitest";
import {
  HOOK_KEY, LIMITS, OVERRIDE_COOKIE, REV_EXPR, SNAPSHOT_EXPR, type DevtoolsHook, type OverrideStore, type Value,
  cookieName, cookieStore, isDevOrigin, parseSetResult, parseSnapshot, registerDevSettings, resetExpr, setExpr,
} from "./devtools";
import { changelingStyle, feedbackAxes } from "./devtools-fixtures";

/** A page at `href`, with a cookie jar that keeps one value per name, as a browser's does for
 * one host and path; pages on one host share it whatever their ports (`jar`), as cookies do. */
function page(href: string, jar = new Map<string, string>()) {
  const u = new URL(href);
  const doc = {
    get cookie() { return [...jar].map(([k, v]) => `${k}=${v}`).join("; "); },
    set cookie(c: string) {
      const [kv, ...attrs] = c.split("; ");
      const [k, v] = [kv.slice(0, kv.indexOf("=")), kv.slice(kv.indexOf("=") + 1)];
      if (attrs.includes("max-age=0")) jar.delete(k); else jar.set(k, v);
    },
  };
  return { location: { origin: u.origin, search: u.search, port: u.port }, document: doc, jar } as unknown as Window & typeof globalThis & { jar: Map<string, string> };
}
const hookOf = (w: object) => (w as Record<string, DevtoolsHook>)[HOOK_KEY];
/** Evaluates a panel expression in `w`, as `inspectedWindow.eval` would. */
const run = (w: object, expr: string): unknown => new Function("window", `return ${expr};`)(w);
function memory(): OverrideStore & { data: Map<string, Value> } {
  const data = new Map<string, Value>();
  return { data, get: k => data.get(k), set: (k, v) => { data.set(k, v); return true; }, delete: k => { data.delete(k); } };
}
afterEach(() => { delete (globalThis as Record<string, unknown>)[HOOK_KEY]; });

describe("isDevOrigin", () => {
  it("takes localhost, loopback and the .localhost/.local/.test names", () => {
    for (const o of ["http://localhost:7700", "http://127.0.0.1:7700", "http://[::1]:5173", "https://changeling.localhost", "http://phobos.local", "https://app.test"]) expect(isDevOrigin(o), o).toBe(true);
  });
  it("refuses public names, other schemes and junk", () => {
    for (const o of ["https://changeling.io", "https://phobos.tail556f5b.ts.net", "https://localhost.example.com", "chrome-extension://x", "file:///tmp/a", "not a url"]) expect(isDevOrigin(o), o).toBe(false);
  });
});

describe("registerDevSettings", () => {
  it("installs nothing off a dev origin, unless the host allows it; never when it says no", () => {
    const prod = page("https://changeling.io/");
    expect(registerDevSettings(changelingStyle, { win: prod })).toBeNull();
    expect(hookOf(prod)).toBeUndefined();
    expect(registerDevSettings(changelingStyle, { win: page("http://127.0.0.1:7700/"), allow: false })).toBeNull();
    expect(registerDevSettings(changelingStyle, { win: prod, allow: true })).not.toBeNull();
    expect(hookOf(prod).v).toBe(1);
  });

  it("shows each field's effective value and its source: the base until the viewer overrides it", () => {
    const w = page("http://127.0.0.1:7700/");
    const store = memory();
    const h = registerDevSettings(feedbackAxes, { win: w, store })!;
    const snap = hookOf(w).snapshot();
    expect(snap.origin).toBe("http://127.0.0.1:7700");
    const pres = snap.sections[0].fields.find(f => f.id === "presentation")!;
    expect(pres).toMatchObject({ value: "dialog", source: "clax", applies: "reload" });
    expect(pres.inherited).toBeUndefined();
    expect(h.source("entry-header")).toBe("site");
  });

  it("keeps a set in the store, says it is the viewer's with what it would inherit, tells the page, and bumps rev", () => {
    const w = page("http://localhost:7700/");
    const store = memory();
    const told: unknown[] = [];
    registerDevSettings({ ...feedbackAxes, onChange: (...a) => told.push(a) }, { win: w, store });
    const hook = hookOf(w);
    const rev = hook.rev;
    const r = hook.set("clax", "presentation", "drawer");
    expect(r).toMatchObject({ ok: true, field: { value: "drawer", source: "you", inherited: { value: "dialog", source: "clax" } } });
    expect(store.data.get("clax.presentation")).toBe("drawer");
    expect(told).toEqual([["presentation", "drawer", "you"]]);
    expect(hook.rev).toBeGreaterThan(rev);
    expect(hook.reset("clax", "presentation")).toMatchObject({ ok: true, field: { value: "dialog", source: "clax" } });
    expect(store.data.has("clax.presentation")).toBe(false);
    expect(told.at(-1)).toEqual(["presentation", "dialog", "clax"]);
  });

  it("refuses a value the field does not take, a disabled option, a locked field and an unknown one, changing nothing", () => {
    const w = page("http://localhost/");
    const store = memory();
    registerDevSettings(feedbackAxes, { win: w, store });
    registerDevSettings(changelingStyle, { win: w, store });
    const hook = hookOf(w);
    expect(hook.set("clax", "presentation", "popover").ok).toBe(false);
    expect(hook.set("clax", "enabled", "yes").ok).toBe(false);
    expect(hook.set("clax", "note", "x".repeat(81)).ok).toBe(false);
    expect(hook.set("feedback", "style", "drawer").ok).toBe(false);
    expect(hook.set("clax", "attach-logs", true)).toMatchObject({ ok: false, reason: expect.stringMatching(/Clax config/) });
    expect(hook.set("nope", "x", true).ok).toBe(false);
    expect(store.data.size).toBe(0);
  });

  it("ignores a stored override the field no longer takes, and any on a locked field", () => {
    const w = page("http://localhost/");
    const store = memory();
    store.data.set("clax.presentation", "popover");
    store.data.set("clax.attach-logs", true);
    const h = registerDevSettings(feedbackAxes, { win: w, store })!;
    expect([h.value("presentation"), h.source("presentation")]).toEqual(["dialog", "clax"]);
    expect([h.value("attach-logs"), h.source("attach-logs")]).toEqual([false, "site"]);
  });

  it("keeps an override set by a `?clax.<section>.<field>=` link; an empty value resets it", () => {
    const store = memory();
    registerDevSettings(feedbackAxes, { win: page("http://localhost/?clax.clax.presentation=sheet&clax.clax.entry-key=false&clax.clax.attach-logs=true&clax.other.x=1"), store });
    expect(Object.fromEntries(store.data)).toEqual({ "clax.presentation": "sheet", "clax.entry-key": false });
    registerDevSettings(feedbackAxes, { win: page("http://localhost/?clax.clax.presentation="), store });
    expect(store.data.has("clax.presentation")).toBe(false);
  });

  it("lets the page move a base, the viewer's override still winning", () => {
    const w = page("http://localhost/");
    const h = registerDevSettings(feedbackAxes, { win: w, store: memory() })!;
    hookOf(w).set("clax", "presentation", "window");
    h.setBase("presentation", { value: "sheet", source: "site" });
    expect(hookOf(w).snapshot().sections[0].fields.find(f => f.id === "presentation")).toMatchObject({ value: "window", source: "you", inherited: { value: "sheet", source: "site" } });
  });

  it("registers into a hook another copy of the SDK installed, and a disposed section leaves", () => {
    const w = page("http://localhost/");
    registerDevSettings(changelingStyle, { win: w, store: memory() });
    const first = hookOf(w);
    const h = registerDevSettings(feedbackAxes, { win: w, store: memory() })!;
    expect(hookOf(w)).toBe(first);
    expect(first.snapshot().sections.map(s => s.id)).toEqual(["feedback", "clax"]);
    h.dispose();
    expect(first.snapshot().sections.map(s => s.id)).toEqual(["feedback"]);
  });

  it("refuses ids the protocol cannot carry", () => {
    expect(() => registerDevSettings({ ...feedbackAxes, id: "a.b" }, { win: page("http://localhost/"), store: memory() })).toThrow(/section id/);
  });
});

describe("cookieStore", () => {
  it("keeps every override in one clax_dev cookie, and clears it when the last goes", () => {
    const w = page("http://localhost/");
    const s = cookieStore(w.document, OVERRIDE_COOKIE);
    expect(s.set("clax.presentation", "drawer")).toBe(true);
    expect(s.set("clax.enabled", false)).toBe(true);
    expect(JSON.parse(decodeURIComponent(w.jar.get(OVERRIDE_COOKIE)!))).toEqual({ "clax.presentation": "drawer", "clax.enabled": false });
    expect(cookieStore(w.document, OVERRIDE_COOKIE).get("clax.enabled")).toBe(false);
    s.delete("clax.presentation");
    s.delete("clax.enabled");
    expect(w.jar.has(OVERRIDE_COOKIE)).toBe(false);
  });
  it("names the cookie by port, so dev apps on one host (which share a jar) keep their own overrides", () => {
    expect([cookieName({ port: "7700" }), cookieName({ port: "" })]).toEqual(["clax_dev_7700", "clax_dev"]);
    const jar = new Map<string, string>();
    const changeling = page("http://127.0.0.1:7700/", jar);
    const other = page("http://127.0.0.1:5173/", jar);
    registerDevSettings(feedbackAxes, { win: changeling });
    const h = registerDevSettings(feedbackAxes, { win: other })!;
    hookOf(changeling).set("clax", "presentation", "drawer");
    expect([...jar.keys()]).toEqual(["clax_dev_7700"]);
    expect([h.value("presentation"), h.source("presentation")]).toEqual(["dialog", "clax"]);
  });
  it("keeps nothing past its size cap, and reads a broken cookie as empty", () => {
    const w = page("http://localhost/");
    const s = cookieStore(w.document, OVERRIDE_COOKIE);
    expect(s.set("clax.note", "x".repeat(4000))).toBe(false);
    w.jar.set(OVERRIDE_COOKIE, "%7Bnot json");
    expect(s.get("clax.note")).toBeUndefined();
  });
});

describe("the panel's side", () => {
  it("reads rev and the snapshot, and sets and resets, through the expressions it evaluates", () => {
    const w = page("http://localhost/");
    registerDevSettings(changelingStyle, { win: w, store: memory() });
    expect(typeof run(w, REV_EXPR)).toBe("number");
    const snap = parseSnapshot(run(w, SNAPSHOT_EXPR));
    expect(snap?.sections[0].fields[0].options?.find(o => o.value === "drawer")).toMatchObject({ disabled: true, needs: '`presentation: "drawer"`' });
    expect(parseSetResult(run(w, setExpr("feedback", "style", "floating")))).toMatchObject({ ok: true, field: { value: "floating", source: "you" } });
    expect(parseSetResult(run(w, resetExpr("feedback", "style")))).toMatchObject({ ok: true, field: { value: "native", source: "site" } });
  });

  it("evaluates to null on a page without a hook, or with a hook that throws", () => {
    expect(run({}, REV_EXPR)).toBeNull();
    expect(run({ [HOOK_KEY]: { v: 1, snapshot() { throw new Error("x"); } } }, SNAPSHOT_EXPR)).toBeNull();
    expect(run({ [HOOK_KEY]: { v: 2, rev: 1 } }, REV_EXPR)).toBeNull();
  });

  it("carries a hostile value as a JSON literal, never as code", () => {
    const w = page("http://localhost/");
    registerDevSettings(feedbackAxes, { win: w, store: memory() });
    const evil = `"); window.pwned = 1; ("`;
    expect(parseSetResult(run(w, setExpr("clax", "note", evil)))).toMatchObject({ ok: true, field: { value: evil } });
    expect(run(w, setExpr(`a"); window.pwned = 1; ("`, "f", true))).toBe(JSON.stringify({ ok: false, reason: "No such setting." }));
    expect((w as unknown as Record<string, unknown>).pwned).toBeUndefined();
  });

  it("refuses a snapshot that is off in any way", () => {
    const good = { v: 1, rev: 1, origin: "http://localhost", sections: [{ id: "a", title: "A", fields: [{ id: "f", label: "F", kind: "toggle", value: true, source: "site", applies: "live" }] }] };
    expect(parseSnapshot(good)).not.toBeNull();
    const bad = (patch: (g: typeof good) => unknown) => parseSnapshot(patch(structuredClone(good)));
    expect(bad(g => ({ ...g, v: 2 }))).toBeNull();
    expect(bad(g => { (g.sections[0].fields[0] as Record<string, unknown>).value = "yes"; return g; })).toBeNull();
    expect(bad(g => { (g.sections[0].fields[0] as Record<string, unknown>).kind = "html"; return g; })).toBeNull();
    expect(bad(g => { (g.sections[0].fields[0] as Record<string, unknown>).source = "admin"; return g; })).toBeNull();
    expect(bad(g => { g.sections[0].id = "<b>"; return g; })).toBeNull();
    expect(bad(g => { g.sections[0].title = "x".repeat(LIMITS.label + 1); return g; })).toBeNull();
    expect(bad(g => { (g.sections[0].fields[0] as Record<string, unknown>).kind = "radio"; (g.sections[0].fields[0] as Record<string, unknown>).value = "a"; return g; })).toBeNull();
    expect(parseSnapshot("x".repeat(LIMITS.json + 1))).toBeNull();
    expect(parseSnapshot("{")).toBeNull();
  });

  it("drops what it does not know of a field (no markup or extra keys cross)", () => {
    const s = parseSnapshot({ v: 1, rev: 0, origin: "o", sections: [{ id: "a", title: "A", extra: 1, fields: [{ id: "f", label: "F", kind: "text", value: "", source: "you", applies: "reload", inherited: { value: "x", source: "clax" }, onclick: "x" }] }] });
    expect(s?.sections[0]).toEqual({ id: "a", title: "A", fields: [{ id: "f", label: "F", kind: "text", value: "", source: "you", applies: "reload", inherited: { value: "x", source: "clax" } }] });
  });
});
