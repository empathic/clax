import { beforeAll, describe, expect, it } from "vitest";
import { BLOCKED_MAX } from "../src/protocol";
import { settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a stand-in shell: what it tells the shell of the
// lockdown's refusals (`clax:blocked`), and when.
type Msg = { type: string; directive?: string; origin?: string };
const posted: { m: Msg; to: string }[] = [];
const shell = { postMessage: (m: Msg, to: string) => { posted.push({ m, to }); } };
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const blocked = () => posted.filter(p => p.m.type === "clax:blocked").map(p => p.m);

const LOCKDOWN = "frame-ancestors 'self' http://localhost:7480; default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; style-src 'self' 'unsafe-inline' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'";
/** A violation as the browser fires it at the document (jsdom has no SecurityPolicyViolationEvent). */
function violate(blockedURI: string, effectiveDirective = "script-src-elem", originalPolicy = LOCKDOWN, trustedEvent = true) {
  const e = new Event("securitypolicyviolation", { bubbles: true, composed: true });
  Object.assign(e, { blockedURI, effectiveDirective, originalPolicy, disposition: "enforce" });
  if (trustedEvent) dispatchTrusted(document, e); else document.dispatchEvent(e);
}

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html"; script.dataset.origin = location.origin;
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
});

describe("the lockdown's refusals", () => {
  it("are kept until the shell welcomes the page, then told to it alone, each directive and origin once, and all again after a welcome", async () => {
    violate("https://cdn.example.com/a.js?secret=1");
    violate("https://cdn.example.com/b.js");
    expect(blocked()).toEqual([]);
    send({ type: "clax:welcome", mode: "view" });
    await settle();
    expect(blocked()).toEqual([{ type: "clax:blocked", directive: "script-src-elem", origin: "https://cdn.example.com" }]);
    expect(posted.filter(p => p.m.type === "clax:blocked").every(p => p.to === location.origin)).toBe(true);
    // After the welcome, each new one at once; a duplicate, a page's own meta policy, or a forged event, never.
    violate("https://fonts.example/f.woff2", "font-src");
    violate("https://fonts.example/g.woff2", "font-src");
    violate("https://elsewhere.example/x.js", "script-src-elem", "script-src 'self'");
    violate("https://forged.example/x.js", "script-src-elem", LOCKDOWN, false);
    expect(blocked().map(m => `${m.directive} ${m.origin}`)).toEqual(["script-src-elem https://cdn.example.com", "font-src https://fonts.example"]);
    // A welcome again (after the shell's greet) hears every one again.
    posted.length = 0;
    send({ type: "clax:welcome", mode: "view" });
    await settle();
    expect(blocked().map(m => m.origin)).toEqual(["https://cdn.example.com", "https://fonts.example"]);
  });

  it("stop at BLOCKED_MAX a document, however many the page makes", () => {
    posted.length = 0;
    for (let i = 0; i < BLOCKED_MAX * 5; i++) violate(`https://h${i}.example/x.png`, "img-src");
    expect(blocked()).toHaveLength(BLOCKED_MAX - 2);
  });
});
