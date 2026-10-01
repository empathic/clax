// The theme's topbar rules reach the artifact view's controls, which render
// inside the topbar's island: a selector that stops matching through the
// island would drop their style without failing anything else.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";

const css = readFileSync(join(__dirname, "theme.css"), "utf8");

const ID = "7q3k9mzx2b4t";
class FakeES { addEventListener() {} close() {} }
const loaded = { artifact: { id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: 2, pinned: false }, versions: [{ artifact_id: ID, n: 1, label: null, created_at: "x", files: {} }, { artifact_id: ID, n: 2, label: null, created_at: "x", files: {} }] };

/** theme.css as (selector, declarations) pairs, media rules' inner rules included. */
const RULES = Array.from(css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g), m => ({ selectors: m[1].split(",").map(x => x.trim()), body: m[2] }));

/** The declarations of the rules with a selector matching `el`, or its `::before` when `pseudo`. */
function rulesFor(el: Element, pseudo?: "::before"): string[] {
  // A keyframe or at-rule "selector" is no selector: it matches nothing.
  const matches = (sel: string) => { try { return el.matches(sel); } catch { return false; } };
  return RULES.filter(r => r.selectors.some(sel => pseudo ? sel.endsWith(pseudo) && matches(sel.slice(0, -pseudo.length)) : !sel.includes("::") && matches(sel))).map(r => r.body);
}

afterEach(async () => { (await import("./caps/gesture")).unwatchShell(); vi.unstubAllGlobals(); sessionStorage.clear(); document.head.replaceChildren(); document.body.replaceChildren(); });

it("styles the topbar controls through the island: open raw and copy link bracketed, a pressed Comment in the pin colour", async () => {
  vi.resetModules();
  history.replaceState(null, "", `/a/${ID}`);
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes("/threads") ? { threads: [], next_cursor: null } : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } } : url === "/api/token" ? { token: "tk" } : loaded))));
  Object.defineProperty(navigator, "clipboard", { value: { writeText: async () => {} }, configurable: true });
  sessionStorage.setItem("clax.origin-ok", "0");
  const root = document.createElement("div");
  document.body.append(root);
  const view = (await import("./artifact")).mountArtifactView(root, { id: ID, pinnedVersion: null });
  const deadline = Date.now() + 2000;
  while (!root.querySelector(".topbar a.hide-sm") && Date.now() < deadline) await new Promise(r => setTimeout(r, 10));
  const raw = root.querySelector(".topbar a.hide-sm")!;
  const buttons = Array.from(root.querySelectorAll<HTMLButtonElement>(".topbar button"));
  const copy = buttons.find(b => b.textContent === "copy link")!;
  const comment = buttons.find(b => b.textContent === "Comment")!;
  for (const el of [raw, copy]) {
    expect(rulesFor(el).some(b => /text-transform:\s*uppercase/.test(b)), el.textContent!).toBe(true);
    expect(rulesFor(el, "::before").some(b => /content:\s*"\[/.test(b)), el.textContent!).toBe(true);
  }
  comment.click();
  while (comment.getAttribute("aria-pressed") !== "true" && Date.now() < deadline) await new Promise(r => setTimeout(r, 10));
  expect(rulesFor(comment).some(b => /background:\s*var\(--pin\)/.test(b))).toBe(true);
  view.unmount();
});
