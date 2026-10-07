// The theme's top bar rules reach the artifact view's controls, which render
// inside the topbar's island: a selector that stops matching through the
// island would drop their style without failing anything else.
import { readFileSync } from "node:fs";
import { FakeWorker } from "./test/fake-worker";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./test/timeouts";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";

const css = readFileSync(join(__dirname, "theme.css"), "utf8");

const ID = "7q3k9mzx2b4t";
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

it("styles the topbar controls through the island: a 3px red-orange rule under the bar in comment mode, a pressed Comment in the people's tint and ink, and no case transform", { timeout: MOUNT_TIMEOUT_MS }, async () => {
  vi.resetModules();
  history.replaceState(null, "", `/a/${ID}`);
  vi.stubGlobal("SharedWorker", FakeWorker);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(
    url.includes("/threads") ? { threads: [], next_cursor: null } : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } } : url === "/api/token" ? { token: "tk" } : loaded))));
  Object.defineProperty(navigator, "clipboard", { value: { writeText: async () => {} }, configurable: true });
  sessionStorage.setItem("clax.origin-ok", "0");
  const root = document.createElement("div");
  document.body.append(root);
  const view = (await import("./artifact")).mountArtifactView(root, { id: ID, pinnedVersion: null });
  const deadline = Date.now() + WAIT_MS;
  while (!root.querySelector(".topbar button.comment") && Date.now() < deadline) await new Promise(r => setTimeout(r, 10));
  const topbar = root.querySelector<HTMLElement>(".topbar")!;
  const comment = root.querySelector<HTMLButtonElement>(".topbar button.comment")!;
  expect(comment.closest(".topbar > .island")).not.toBeNull();
  comment.click();
  while ((comment.getAttribute("aria-pressed") !== "true" || !topbar.classList.contains("commenting")) && Date.now() < deadline) await new Promise(r => setTimeout(r, 10));
  expect(rulesFor(topbar).some(b => /box-shadow:\s*inset 0 -3px 0 var\(--you\)/.test(b))).toBe(true);
  expect(rulesFor(comment).some(b => /background:\s*var\(--comment-hl\)/.test(b) && /color:\s*var\(--you-ink\)/.test(b))).toBe(true);
  const inIsland = Array.from(topbar.querySelectorAll(".island *"));
  expect(inIsland.length).toBeGreaterThan(0);
  for (const el of inIsland) expect(rulesFor(el).some(b => /text-transform/.test(b)), el.outerHTML.slice(0, 60)).toBe(false);
  view.unmount();
});
