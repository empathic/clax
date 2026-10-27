// The quickstart page's own script (web/quickstart/index.html), run under
// jsdom with a stubbed referrer and `window.claude`: a signed-in person on
// clax.dev has step 1 ticked and is told so. The page runs on its artifact's
// own site, so clax.dev is known from the shell's origin, the referrer of the
// first load in the tab, which sessionStorage keeps for loads inside the frame.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const html = readFileSync(join(__dirname, "../quickstart/index.html"), "utf8");
const doc = new DOMParser().parseFromString(html, "text/html");
const script = [...doc.querySelectorAll("script")].find((s) => !s.src)!.textContent!;
const body = (() => {
  const b = doc.body.cloneNode(true) as HTMLElement;
  b.querySelectorAll("script").forEach((s) => s.remove());
  return b.innerHTML;
})();

/** A viewer the `user` capability names: a person (`guest: false`) or a guest. */
const userOf = (guest: boolean) => ({
  me: async () => ({ id: "u1", name: "ada" }),
  profiles: async () => ({ u1: { name: "Ada Lovelace", guest } }),
  id: async () => "u1",
});

/** Runs the page as loaded with `referrer`, for a viewer `user` names. */
function load(referrer: string, user = userOf(false)) {
  document.body.innerHTML = body;
  Object.defineProperty(document, "referrer", { value: referrer, configurable: true });
  vi.stubGlobal("claude", {
    use: async (name: string) => {
      if (name === "user") return user;
      throw new Error(`no ${name}`);
    },
  });
  new Function(script)();
}

const toast = () => document.getElementById("toast")!;
const saved = () => JSON.parse(localStorage.getItem("clax-quickstart") ?? "null");
/** Lets the page's capability calls settle: they are promises alone, no timers. */
const settle = async () => { for (let i = 0; i < 100; i++) await Promise.resolve(); };

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  vi.stubGlobal("ResizeObserver", class { observe() {} });
  Element.prototype.scrollIntoView = () => {};
});
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("the quickstart notices a sign-in", () => {
  it("ticks_sign_in_and_says_so_for_a_person_on_clax_dev", async () => {
    load("https://clax.dev/a/abcd1234");
    await settle();
    expect(toast().hidden).toBe(false);
    expect(toast().textContent).toBe("Signed in as Ada Lovelace — on to step 2");
    expect(saved().done.signin).toBe(true);
    expect(document.querySelector('[data-id="signin"] .fab')!.classList.contains("on")).toBe(true);
  });

  it("keeps_the_shell_origin_after_the_frame_navigates", async () => {
    sessionStorage.setItem("clax-quickstart-shell", "https://clax.dev");
    // A load inside the frame (back from ideas.html): the referrer is this site.
    load(`${location.origin}/ideas.html`);
    await settle();
    expect(saved()?.done?.signin).toBe(true);
  });

  it("leaves_sign_in_alone_on_a_local_clax", async () => {
    load("http://localhost:7480/a/abcd1234");
    await settle();
    expect(toast().hidden).toBe(true);
    expect(saved()?.done?.signin).toBeUndefined();
    expect(sessionStorage.getItem("clax-quickstart-shell")).toBe("http://localhost:7480");
  });

  it("leaves_sign_in_alone_for_a_guest", async () => {
    load("https://clax.dev/a/abcd1234", userOf(true));
    await settle();
    expect(toast().hidden).toBe(true);
    expect(saved()?.done?.signin).toBeUndefined();
  });
});
