import { afterEach, describe, expect, it } from "vitest";
import { encodeDiagnostics, installDiagnostics, MAX_DIAG_BYTES, MAX_READ_CHARS, MAX_TEXT_CHARS, type Recorder } from "./diagnostics";
import { REDACTED } from "./scrub";
import type { DiagLine } from "./types";

const T0 = Date.parse("2026-10-29T07:12:03.120Z");

/** A fake XMLHttpRequest whose `send` answers with the class's `next` status. */
class FakeXhr extends EventTarget {
  static next: { status: number; headers?: Record<string, string>; fail?: boolean } = { status: 200 };
  status = 0;
  private headers: Record<string, string> = {};
  open(_method: string, _url: string) {}
  setRequestHeader(_name: string, _value: string) {}
  getResponseHeader(name: string): string | null {
    return this.headers[name.toLowerCase()] ?? null;
  }
  send(_body?: unknown) {
    const n = FakeXhr.next;
    this.headers = n.headers ?? {};
    if (n.fail) this.dispatchEvent(new Event("error"));
    else this.status = n.status;
    this.dispatchEvent(new Event("loadend"));
  }
}

/** A window with its own console, fetch and XMLHttpRequest, and a clock. */
function fakeWindow(answer: (url: string, init?: RequestInit) => Promise<Response> = async () => new Response("ok")) {
  let now = T0;
  const logged: unknown[][] = [];
  const target = new EventTarget();
  const fetched: string[] = [];
  const win = {
    console: { warn: (...a: unknown[]) => logged.push(["warn", ...a]), error: (...a: unknown[]) => logged.push(["error", ...a]) },
    fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      fetched.push(url);
      const res = answer(url, init);
      now += 42;
      return res;
    },
    XMLHttpRequest: class extends FakeXhr {},
    location: { href: "https://app.example/orders/7?tab=2" },
    addEventListener: target.addEventListener.bind(target),
    removeEventListener: target.removeEventListener.bind(target),
    dispatchEvent: target.dispatchEvent.bind(target),
  };
  return { win, w: win as unknown as Window, logged, fetched, clock: () => now, tick: (ms: number) => { now += ms; } };
}

const installed: Recorder[] = [];
function install(...args: Parameters<typeof installDiagnostics>): Recorder {
  const r = installDiagnostics(...args);
  installed.push(r);
  return r;
}
afterEach(() => {
  for (const r of installed.splice(0)) r.stop();
});

describe("installDiagnostics", () => {
  it("keeps the last 200", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    for (let i = 0; i < 205; i++) f.win.console.warn(`warning ${i}`);
    expect(rec.count()).toBe(200);
    const lines = rec.lines();
    expect(lines[0].text).toBe("warning 5");
    expect(lines[199].text).toBe("warning 204");
    // A smaller ring when asked; never a larger one.
    const g = fakeWindow();
    const small = install(g.w, { now: g.clock, max: 3 });
    for (let i = 0; i < 5; i++) g.win.console.warn(`w${i}`);
    expect(small.lines().map(l => l.text)).toEqual(["w2", "w3", "w4"]);
    const h = fakeWindow();
    const big = install(h.w, { now: h.clock, max: 1000 });
    for (let i = 0; i < 205; i++) h.win.console.warn("x");
    expect(big.count()).toBe(200);
  });

  it("records console.error and still calls through", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    const err = new TypeError("bad total for ahk@example.com");
    err.stack = "TypeError: bad total for ahk@example.com\n    at total (https://app.example/a.js:1:2)";
    f.win.console.error("render failed", err);
    f.win.console.warn("token=s3cret");
    expect(f.logged).toEqual([["error", "render failed", err], ["warn", "token=s3cret"]]);
    expect(rec.lines()).toEqual([
      { at: "2026-10-29T07:12:03.120Z", level: "error", kind: "console", text: `render failed TypeError: bad total for ${REDACTED}\n    at total (https://app.example/a.js:1:2)` },
      { at: "2026-10-29T07:12:03.120Z", level: "warn", kind: "console", text: `token=${REDACTED}` },
    ]);
    f.win.console.warn("x".repeat(MAX_TEXT_CHARS + 50));
    const long = rec.lines()[2].text;
    expect(long).toHaveLength(MAX_TEXT_CHARS);
    expect(long.endsWith("…")).toBe(true);
  });

  it("records a failed fetch with scrubbed path and request ID", async () => {
    const f = fakeWindow(async url => {
      if (url.includes("/ok")) return new Response("fine");
      if (url.includes("/down")) throw new TypeError("Failed to fetch");
      if (url.includes("/gone")) throw new DOMException("aborted", "AbortError");
      return new Response("no", { status: url.includes("other") ? 404 : 502, headers: { "x-request-id": "req-7f3a" } });
    });
    const rec = install(f.w, { now: f.clock });
    expect((await f.win.fetch("/ok")).status).toBe(200);
    expect((await f.win.fetch("/api/orders/7?token=s3cret&n=2", { method: "post" })).status).toBe(502);
    await f.win.fetch("https://other.example/x?q=me");
    await expect(f.win.fetch("/down")).rejects.toThrow("Failed to fetch");
    await expect(f.win.fetch("/gone")).rejects.toThrow("aborted");
    expect(rec.lines()).toEqual([
      { at: expect.any(String), level: "error", kind: "request", text: `POST /api/orders/7?token=${REDACTED}&n=${REDACTED} → 502 in 42 ms [req req-7f3a]` },
      { at: expect.any(String), level: "warn", kind: "request", text: `GET https://other.example/x?q=${REDACTED} → 404 in 42 ms [req req-7f3a]` },
      { at: expect.any(String), level: "error", kind: "request", text: "GET /down → failed in 42 ms" },
    ]);
  });

  it("keeps text well-formed and cuts it on a code point", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    // An astral character at 998–999 straddles the cut at 999.
    f.win.console.warn(`${"x".repeat(MAX_TEXT_CHARS - 2)}😀${"y".repeat(10)}`);
    f.win.console.warn("half \ud83d pair");
    const [cutLine, lone] = rec.lines();
    expect(cutLine.text).toBe(`${"x".repeat(MAX_TEXT_CHARS - 2)}…`);
    expect(lone.text).toBe("half � pair");
    const sent = new TextDecoder().decode(encodeDiagnostics([...rec.lines(), { at: "t", level: "info", kind: "console", text: "raw \udc00" }]));
    expect(sent).not.toMatch(/\\u[dD][89a-fA-F]/);
    expect(sent).toContain("raw �");
    // A huge value is read only in part.
    f.win.console.error("z".repeat(10 * MAX_READ_CHARS));
    expect(rec.lines()[2].text).toHaveLength(MAX_TEXT_CHARS);
  });

  it("records each request of a reused XMLHttpRequest once", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    const x = new f.win.XMLHttpRequest();
    const added: string[] = [];
    const add = x.addEventListener.bind(x);
    x.addEventListener = ((type: string, l: EventListenerOrEventListenerObject) => { added.push(type); add(type, l); }) as typeof x.addEventListener;
    for (const [url, status] of [["/a", 500], ["/b", 200], ["/c", 404], ["https://clax.example/x", 500]] as const) {
      FakeXhr.next = { status };
      x.open("GET", url);
      x.send();
    }
    expect(rec.lines().map(l => l.text)).toEqual(["GET /a → 500 in 0 ms", "GET /c → 404 in 0 ms", "GET https://clax.example/x → 500 in 0 ms"]);
    expect(added).toEqual(["error", "timeout", "loadend"]);
  });

  it("records nothing once stopped, under another script's wrapper too", async () => {
    const f = fakeWindow(async () => new Response("no", { status: 500 }));
    const rec = install(f.w, { now: f.clock });
    const ours = f.win.console.warn;
    const theirs = (...a: unknown[]) => ours(...a);
    f.win.console.warn = theirs;
    const proto = f.win.XMLHttpRequest.prototype;
    const ourSend = proto.send;
    const theirSend = function (this: XMLHttpRequest, b?: unknown) { return ourSend.call(this, b as never); } as typeof proto.send;
    proto.send = theirSend;
    rec.stop();
    expect(f.win.console.warn).toBe(theirs);
    expect(proto.send).toBe(theirSend);
    f.win.console.warn("after");
    await f.win.fetch("/x");
    FakeXhr.next = { status: 500 };
    const x = new f.win.XMLHttpRequest();
    x.open("GET", "/y");
    x.send();
    expect(rec.count()).toBe(0);
    expect(f.logged.at(-1)).toEqual(["warn", "after"]);
  });

  it("records a failed XMLHttpRequest", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    const Xhr = f.win.XMLHttpRequest;
    const send = (method: string, url: string, next: typeof FakeXhr.next) => {
      FakeXhr.next = next;
      const x = new Xhr();
      x.open(method, url);
      x.send();
    };
    send("GET", "/fine", { status: 204 });
    send("PUT", "/api/x?secret=1", { status: 500, headers: { "cf-ray": "8a1b-SJC" } });
    send("GET", "/offline", { status: 0, fail: true });
    expect(rec.lines().map(l => [l.level, l.text])).toEqual([
      ["error", `PUT /api/x?secret=${REDACTED} → 500 in 0 ms [req 8a1b-SJC]`],
      ["error", "GET /offline → failed in 0 ms"],
    ]);
  });

  it("records an unhandled rejection", () => {
    const f = fakeWindow();
    const rec = install(f.w, { now: f.clock });
    const ev = Object.assign(new Event("unhandledrejection"), { reason: new Error("lost cxr_0123456789abcdefABCD") });
    f.win.dispatchEvent(ev);
    f.win.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: { code: 7 } }));
    f.win.dispatchEvent(Object.assign(new Event("error"), { message: "Script error.", filename: "https://app.example/a.js?v=abc", lineno: 3, colno: 9 }));
    const [a, b, c] = rec.lines();
    expect(a.kind).toBe("rejection");
    expect(a.level).toBe("error");
    expect(a.text.startsWith(`Error: lost ${REDACTED}`)).toBe(true);
    expect(b.text).toBe('{"code":7}');
    expect(c).toMatchObject({ kind: "error", text: `Script error. (https://app.example/a.js?v=${REDACTED}:3:9)` });
  });

  it("second install returns the same recorder", () => {
    const f = fakeWindow();
    const first = install(f.w, { now: f.clock });
    expect(installDiagnostics(f.w)).toBe(first);
    f.win.console.warn("once");
    expect(first.count()).toBe(1);
    first.stop();
    // Stopped, it gives the window back and the next install starts afresh.
    f.win.console.warn("after");
    expect(f.logged.at(-1)).toEqual(["warn", "after"]);
    const again = install(f.w, { now: f.clock });
    expect(again).not.toBe(first);
    expect(again.count()).toBe(0);
  });

  it("skips the instance's own requests", async () => {
    const f = fakeWindow(async () => new Response("no", { status: 401 }));
    const rec = install(f.w, { now: f.clock, instance: "https://clax.example:8443" });
    await f.win.fetch("https://clax.example:8443/api/queues/q1/reporter/badge");
    // Any signed request is the SDK's, wherever it goes.
    await f.win.fetch("https://elsewhere.example/api/x", { headers: { "Clax-Signature": "t=1" } });
    // An instance named by a later install is skipped too.
    installDiagnostics(f.w, { instance: "https://second.example" });
    await f.win.fetch("https://second.example/x");
    await f.win.fetch("/mine");
    expect(rec.lines().map(l => l.text)).toEqual(["GET /mine → 401 in 42 ms"]);
    expect(f.fetched).toHaveLength(4);
  });
});

describe("encodeDiagnostics", () => {
  it("sends the newest lines that fit, as JSON lines", () => {
    const line = (text: string): DiagLine => ({ at: "2026-10-29T07:12:03.120Z", level: "info", kind: "console", text });
    const text = new TextDecoder().decode(encodeDiagnostics([line("a"), line("b")]));
    expect(text.split("\n")).toEqual([JSON.stringify(line("a")), JSON.stringify(line("b")), ""]);
    const many = Array.from({ length: 250 }, (_, i) => line(String(i)));
    const kept = new TextDecoder().decode(encodeDiagnostics(many)).trim().split("\n");
    expect(kept).toHaveLength(200);
    expect(JSON.parse(kept[0]).text).toBe("50");
    const big = Array.from({ length: 200 }, () => line("é".repeat(1000)));
    const bytes = encodeDiagnostics(big);
    expect(bytes.length).toBeLessThanOrEqual(MAX_DIAG_BYTES);
    expect(new TextDecoder().decode(bytes).trim().split("\n").length).toBeLessThan(200);
  });
});
